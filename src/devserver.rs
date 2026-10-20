//! PRD-mcphost-dev-emulator: `mcphost dev <dir>` serves one tool directory
//! locally through the same `KindRegistry` production uses, with no
//! `AppState` and no database. `pack` + the kind's validators load the
//! directory; a call runs `Kind::call` directly, so every error envelope is
//! the one the kind's own mappers produce (`tool_exception` with
//! `line`/`source_line`, `args_invalid`, ...). Each response carries
//! `emulator: {sandbox, plan}`; the only code the emulator adds is
//! `emulator_unsupported`, for `host.*` verbs it does not serve.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use rmcp::model::CallToolResult;
use serde_json::{Value, json};

use crate::errors::AppError;
use crate::kinds::{CallCtx, Kind, KindError, KindRegistry, describe_args_error};
use crate::plans::{Plan, PlanCatalog};
use crate::sandbox::IsolationMechanism;
use crate::tooldir::{self, TooldirError};

/// Flags `mcphost dev` takes that shape the runtime.
#[derive(Debug, Clone)]
pub struct DevOpts {
    pub dir: PathBuf,
    /// `--no-sandbox`: python runs with `IsolationMechanism::None`.
    pub no_sandbox: bool,
    /// `--plan free|pro`.
    pub plan: String,
}

/// The loaded directory: the published tool's local name, kind and spec.
struct Loaded {
    name: String,
    kind: Arc<dyn Kind>,
    spec: Value,
}

/// A running emulator: the kind registry, the plan table row and the
/// directory's tool.
pub struct Dev {
    dir: PathBuf,
    plan: Plan,
    sandbox: &'static str,
    kinds: KindRegistry,
    loaded: RwLock<Arc<Loaded>>,
    /// `notifications/tools/list_changed` frames for the SSE stream.
    changes: tokio::sync::broadcast::Sender<String>,
}

fn dir_hash(dir: &Path) -> String {
    use sha2::{Digest, Sha256};
    let abs = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    Sha256::digest(abs.to_string_lossy().as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// `$XDG_CACHE_HOME/mcphost-dev/<dir-hash>` (falling back to `~/.cache`).
fn data_dir_for(dir: &Path) -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("mcphost-dev").join(dir_hash(dir))
}

fn invalid(message: impl Into<String>) -> TooldirError {
    TooldirError {
        code: crate::errors::INVALID_SPEC,
        message: message.into(),
        error_code: None,
        data: Value::Null,
    }
}

/// An `AppError` as the JSON-RPC `error` object the host would send.
fn error_json(e: AppError) -> Value {
    serde_json::to_value(e.into_error_data()).unwrap_or(Value::Null)
}

crate::err_code! {
    pub EMULATOR_UNSUPPORTED,
    code: "emulator_unsupported", family: Spec, class: AgentFault, retryable: false,
    remedy: "Publish with mcphost push and call this host.* verb on the host; mcphost dev does not serve it.",
}

/// The one code the emulator adds: a `host.*` verb it does not serve.
fn unsupported(verb: &str) -> Value {
    let hint = format!("publish with mcphost push, then {verb} on the host");
    json!({
        "code": -32601,
        "message": format!("{verb} is not served by mcphost dev; {hint}"),
        "data": {"error_code": EMULATOR_UNSUPPORTED, "verb": verb, "hint": hint},
    })
}

impl Dev {
    /// Packs `opts.dir`, validates it with its kind, and builds the runtime.
    /// Nothing is bound or served here.
    pub async fn open(opts: &DevOpts) -> Result<Self, TooldirError> {
        let catalog = PlanCatalog::default_catalog();
        let plan = catalog
            .get(&opts.plan)
            .cloned()
            .ok_or_else(|| invalid(format!("--plan {}: unknown plan; use free or pro", opts.plan)))?;
        let data_dir = data_dir_for(&opts.dir);
        std::fs::create_dir_all(&data_dir).map_err(|e| invalid(format!("{}: {e}", data_dir.display())))?;
        let isolation = opts.no_sandbox.then_some(IsolationMechanism::None);
        let (kinds, sandbox) = crate::kinds::dev_registry(&data_dir, isolation)
            .map_err(|e| invalid(format!("cannot build the kind registry: {e}")))?;
        let loaded = load(&opts.dir, &kinds).await?;
        plan_gates(&loaded, &plan)?;
        let (changes, _) = tokio::sync::broadcast::channel(16);
        Ok(Self { dir: opts.dir.clone(), plan, sandbox, kinds, loaded: RwLock::new(Arc::new(loaded)), changes })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn snapshot(&self) -> Arc<Loaded> {
        self.loaded.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Re-lints and re-packs the directory; on success swaps the tool in and
    /// sends `notifications/tools/list_changed`. A directory that no longer
    /// loads keeps the previous tool serving and returns the finding.
    pub async fn reload(&self) -> Result<(), TooldirError> {
        let loaded = load(&self.dir, &self.kinds).await?;
        plan_gates(&loaded, &self.plan)?;
        *self.loaded.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(loaded);
        let _ = self.changes.send(json!({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"}).to_string());
        Ok(())
    }

    /// What `watch` compares: the bytes of every file `pack` reads.
    fn fingerprint(&self) -> Vec<Option<Vec<u8>>> {
        [tooldir::TOOL_TOML, tooldir::TOOL_PY, tooldir::TOOL_WASM]
            .iter()
            .map(|f| std::fs::read(self.dir.join(f)).ok())
            .collect()
    }

    /// The `emulator` stamp every response carries.
    pub fn stamp(&self) -> Value {
        json!({"sandbox": self.sandbox, "plan": self.plan.name})
    }

    /// One `tools/call`: the JSON-RPC `result` or `error` object, plus the
    /// `emulator` stamp, as `{"result": ..}` / `{"error": ..}`.
    pub async fn call(&self, tool: &str, arguments: Value) -> Value {
        let outcome = match self.dispatch(tool, arguments).await {
            Ok(result) => json!({"result": result}),
            Err(error) => json!({"error": error}),
        };
        let mut out = outcome;
        out["emulator"] = self.stamp();
        out
    }

    async fn dispatch(&self, tool: &str, arguments: Value) -> Result<Value, Value> {
        let loaded = self.snapshot();
        let local = &loaded.name;
        // `host.tool_call` / `host.tool_test` address the tool by `name`,
        // with `args` and (for `host.tool_call`) `budget` beside it.
        let verb = matches!(tool, "host.tool_call" | "host.tool_test");
        if tool.starts_with("host.") && !verb {
            return Err(unsupported(tool));
        }
        let (name, args, budget) = if verb {
            let name = arguments.get("name").and_then(Value::as_str).unwrap_or(local).to_string();
            let args = arguments.get("args").cloned().unwrap_or_else(|| json!({}));
            (name, args, arguments.get("budget").cloned())
        } else {
            (tool.to_string(), arguments, None)
        };
        if name == *local {
            return self.run(args, budget.as_ref(), tool == "host.tool_test").await;
        }
        Err(error_json(AppError::ToolNotFound(name)))
    }

    /// Runs the directory's tool once. `budget` is the `host.tool_call`
    /// budget argument; `test_mode` is `host.tool_test`.
    async fn run(&self, args: Value, budget: Option<&Value>, test_mode: bool) -> Result<Value, Value> {
        let loaded = self.snapshot();
        crate::budget::resolve_with(&self.plan, budget, |_, _, _| Value::Null).map_err(error_json)?;
        if !loaded.kind.validates_own_args() {
            let descriptor = loaded.kind.describe(&loaded.spec);
            if let Ok(validator) = jsonschema::validator_for(&descriptor.input_schema)
                && let Err(e) = validator.validate(&args)
            {
                let args_err = describe_args_error(&e);
                return Err(error_json(AppError::Structured {
                    code: crate::errors::ARGS_INVALID,
                    message: args_err.message(),
                    data: args_err.data(),
                }));
            }
        }
        let mut ctx = CallCtx::for_test(0, "dev");
        ctx.tool_name = Some(loaded.name.clone());
        ctx.test_mode = test_mode;
        ctx.egress_allowed = self.plan.name == "pro";
        ctx.sandbox_ceilings = self.plan.sandbox_ceilings();
        let timeout = loaded.kind.requested_timeout(&loaded.spec).unwrap_or(crate::state::CALL_TIMEOUT);
        ctx.deadline = std::time::Instant::now() + timeout;
        let outcome = tokio::time::timeout(timeout, loaded.kind.call(&loaded.spec, args, &ctx)).await;
        match outcome {
            Err(_) => Err(error_json(AppError::CallTimeout(timeout.as_secs()))),
            Ok(Err(e)) => Err(error_json(AppError::from(e))),
            Ok(Ok(value)) => Ok(serde_json::to_value(CallToolResult::structured(value)).unwrap_or(Value::Null)),
        }
    }

    pub fn registry(&self) -> &KindRegistry {
        &self.kinds
    }

    /// `tools/list`'s `tools`: the directory's tool with the args schema the
    /// host would infer for it.
    fn tools(&self) -> Value {
        let loaded = self.snapshot();
        let d = loaded.kind.describe(&loaded.spec);
        json!([{"name": loaded.name, "description": d.description, "inputSchema": d.input_schema}])
    }

    /// One JSON-RPC request. `None` for a notification (no `id`).
    pub async fn handle_rpc(&self, request: Value) -> Option<Value> {
        let id = request.get("id").cloned()?;
        let params = request.get("params").cloned().unwrap_or(Value::Null);
        let body = match request.get("method").and_then(Value::as_str).unwrap_or_default() {
            "initialize" => json!({"result": {
                "protocolVersion": params.get("protocolVersion").cloned().unwrap_or(json!("2025-06-18")),
                "capabilities": {"tools": {"listChanged": true}},
                "serverInfo": {"name": "mcphost-dev", "version": env!("CARGO_PKG_VERSION")},
            }, "emulator": self.stamp()}),
            "ping" => json!({"result": {}}),
            "tools/list" => json!({"result": {"tools": self.tools()}, "emulator": self.stamp()}),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
                let arguments = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
                self.call(name, arguments).await
            }
            other => json!({"error": {"code": -32601, "message": format!("method not found: {other}")}}),
        };
        let mut out = json!({"jsonrpc": "2.0", "id": id});
        if let (Some(o), Some(b)) = (out.as_object_mut(), body.as_object()) {
            o.extend(b.clone());
        }
        Some(out)
    }
}

/// `GET /mcp`: the server-to-client stream `notifications/tools/list_changed`
/// rides on.
async fn events(
    axum::extract::State(dev): axum::extract::State<Arc<Dev>>,
) -> axum::response::sse::Sse<impl futures_util::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>> {
    let rx = dev.changes.subscribe();
    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(frame) => {
                    return Some((Ok(axum::response::sse::Event::default().event("message").data(frame)), rx));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => return None,
            }
        }
    });
    axum::response::sse::Sse::new(stream)
}

/// Polls the directory's files; a change re-packs within [`WATCH_INTERVAL`].
const WATCH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(200);

fn watch(dev: Arc<Dev>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut seen = dev.fingerprint();
        loop {
            tokio::time::sleep(WATCH_INTERVAL).await;
            let now = dev.fingerprint();
            if now == seen {
                continue;
            }
            seen = now;
            if let Err(finding) = dev.reload().await {
                eprintln!("mcphost dev: reload failed, still serving the previous tool: {}", finding.to_json());
            }
        }
    })
}

async fn rpc(
    axum::extract::State(dev): axum::extract::State<Arc<Dev>>,
    axum::Json(request): axum::Json<Value>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    match dev.handle_rpc(request).await {
        Some(response) => axum::Json(response).into_response(),
        None => axum::http::StatusCode::ACCEPTED.into_response(),
    }
}

/// Binds `127.0.0.1:port` (0 = ephemeral) and serves `/mcp` until the
/// returned task is aborted. Binding is the last step: callers validate the
/// directory first.
pub async fn serve(dev: Arc<Dev>, port: u16) -> std::io::Result<(std::net::SocketAddr, tokio::task::JoinHandle<()>)> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let addr = listener.local_addr()?;
    let watcher = watch(dev.clone());
    let app = axum::Router::new().route("/mcp", axum::routing::post(rpc).get(events)).with_state(dev);
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
        watcher.abort();
    });
    Ok((addr, task))
}

/// The publish-time plan gates `host.tool_publish` applies (`control.rs`):
/// a spec value above the plan's sandbox ceiling, and `network: public` on a
/// plan that is not `pro`. `--plan pro` lifts them.
fn plan_gates(loaded: &Loaded, plan: &Plan) -> Result<(), TooldirError> {
    if loaded.kind.name() != "python" {
        return Ok(());
    }
    if let Some((spec_field, requested, ceiling_field, ceiling)) =
        crate::kinds::python::plan_ceiling_violation(&loaded.spec, plan)
    {
        let e = AppError::plan_ceiling_exceeded(spec_field, requested, ceiling_field, ceiling, &plan.name);
        return Err(invalid(e.to_string()));
    }
    if crate::network_policy::wants_egress(loaded.spec.get("network").and_then(Value::as_str)) && plan.name != "pro" {
        return Err(invalid("network: \"public\"/\"egress\" requires the pro plan; run mcphost dev --plan pro"));
    }
    Ok(())
}

/// Lints `dir` (`mcphost tool lint`), packs it and runs its kind's
/// out-of-process validation. Any failure is the `invalid_spec` finding the
/// caller prints before it binds a port.
async fn load(dir: &Path, kinds: &KindRegistry) -> Result<Loaded, TooldirError> {
    tooldir::lint(dir, kinds)?;
    let payload = tooldir::pack(dir)?;
    let name = payload["name"].as_str().unwrap_or_default().to_string();
    let kind_name = payload["kind"].as_str().unwrap_or_default();
    let kind = kinds
        .get(kind_name)
        .filter(|_| kinds.names().contains(&kind_name))
        .ok_or_else(|| invalid(format!("unknown_kind: '{kind_name}' is not a registered kind")))?;
    let spec = payload["spec"].clone();
    let fail = |e: KindError| invalid(e.to_string());
    kind.validate_async(&spec).await.map_err(fail)?;
    Ok(Loaded { name, kind, spec })
}
