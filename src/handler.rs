//! The `rmcp::ServerHandler` implementation: `initialize`, `list_tools` and
//! `call_tool`. This is the only module that translates between `rmcp` wire
//! types and the plain `serde_json::Value` business logic in `control.rs`
//! and `admin.rs`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use rmcp::ErrorData as McpError;
use rmcp::model::{
    CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::{NotificationContext, RequestContext};
use rmcp::{RoleServer, ServerHandler};
use serde_json::{Map, Value, json};

use crate::auth::{extract_bearer, hash_key};
use crate::db::{Tenant, ToolRow};
use crate::errors::AppError;
use crate::kinds::{
    CallCtx, CallLog, Kind, KindRegistry, NullLog, NullResourceSink, ResourceSink, SecretResolver,
};
use crate::state::{AppState, TOOLS_LIST_TTL_GRACE_SECS, TOOLS_LIST_TTL_MS_STEADY, now_unix};
use crate::{admin, control};

/// Who is making this request, resolved once per request from the bearer
/// key (or its absence).
enum Auth {
    /// No `Authorization` header at all.
    Anonymous,
    /// An `Authorization` header that matches neither the admin key nor any
    /// tenant's key hash.
    Invalid,
    Admin,
    Tenant(Tenant),
}

fn value_to_json_object(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

async fn resolve_auth(state: &AppState, parts: &http::request::Parts) -> Result<Auth, AppError> {
    let Some(key) = extract_bearer(&parts.headers) else {
        return Ok(Auth::Anonymous);
    };
    if let Some(admin_key) = &state.admin_key
        && constant_time_eq(key.as_bytes(), admin_key.as_bytes())
    {
        return Ok(Auth::Admin);
    }
    let hash = hash_key(&key);
    match state.db.find_tenant_by_key_hash(hash).await? {
        Some(t) if t.disabled => Err(AppError::TenantDisabled),
        Some(t) => Ok(Auth::Tenant(t)),
        None => Ok(Auth::Invalid),
    }
}

/// PRD-mcphost-session-key requirement 5's argument-derived fallback:
/// consulted from `call_tool` only when the header path above resolved to
/// `Auth::Anonymous` (no `Authorization` header at all -- requirement 6,
/// the header always wins whenever one is present, valid or not). Kept
/// separate from `resolve_auth` rather than threading call arguments into
/// it: that function's other callers (`list_tools`, and `call_tool`'s own
/// header-only resolution above) have no `tools/call` arguments to give it.
/// Mirrors `resolve_auth`'s header branch exactly -- same disabled/unknown
/// handling -- so an argument-based and header-based key are authorized
/// identically (requirement 5) and refused identically (requirements 7, 8).
async fn resolve_tenant_key_auth(state: &AppState, args: &Value) -> Result<Auth, AppError> {
    let Some(key) = args.get("tenant_key").and_then(Value::as_str) else {
        return Ok(Auth::Anonymous);
    };
    let hash = hash_key(key);
    match state.db.find_tenant_by_key_hash(hash).await? {
        Some(t) if t.disabled => Err(AppError::TenantDisabled),
        Some(t) => Ok(Auth::Tenant(t)),
        None => Ok(Auth::Invalid),
    }
}

fn get_parts(ctx: &RequestContext<RoleServer>) -> Result<&http::request::Parts, McpError> {
    ctx.extensions.get::<http::request::Parts>().ok_or_else(|| {
        AppError::Internal("no HTTP request parts on this call".into()).into_error_data()
    })
}

/// The source IP used for signup rate limiting: the TCP peer address axum's
/// `ConnectInfo` extractor attaches to every incoming request.
fn source_ip(parts: &http::request::Parts) -> String {
    parts
        .extensions
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|ci| ci.0.ip().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn mcp_name_header(parts: &http::request::Parts) -> Option<String> {
    parts
        .headers
        .get("Mcp-Name")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

// ---- static control-plane / admin tool descriptors -----------------------

fn schema(props: Value, required: &[&str]) -> Map<String, Value> {
    value_to_json_object(json!({
        "type": "object",
        "properties": props,
        "required": required,
    }))
}

/// PRD-mcphost-session-key requirement 2 / AC3: every `host.*` descriptor
/// (never `signup`'s or an `admin.*` descriptor -- requirement 10) gains an
/// optional string `tenant_key` property, the key `signup` returned,
/// required only when the connection carries no `Authorization` header.
/// Generated once here and applied to each `host_tools()` entry below
/// rather than hand-edited into ten separate `json!` blocks, which would
/// drift.
fn host_schema(mut props: Value, required: &[&str]) -> Map<String, Value> {
    if let Some(obj) = props.as_object_mut() {
        obj.insert(
            "tenant_key".to_string(),
            json!({
                "type": "string",
                "description": "The key `signup` returned. Required only when this \
                    connection carries no Authorization: Bearer header -- when both \
                    are present, the header wins.",
            }),
        );
    }
    schema(props, required)
}

fn signup_tool() -> Tool {
    Tool::new(
        "signup",
        "Create a tenant and receive a bearer key and namespace. Unauthenticated.",
        schema(
            json!({"name": {"type": "string", "description": "display name"}}),
            &["name"],
        ),
    )
}

/// PRD-mcphost-publish-first-try requirement 1 (AC1): one paragraph per
/// registered kind, each with a complete minimal example `spec` an agent
/// can copy verbatim, built from that kind's own [`crate::kinds::Kind::example`]
/// rather than hand-duplicated here -- so this can't say something a real
/// publish would then reject. Requirement 5: names `host.tool_test` as the
/// dry run to try first. Kept under 1,200 characters total (asserted by
/// `tests/publishfirsttry_ac01_tool_publish_description.rs`) so it stays
/// readable in a `tools/list` response.
fn tool_publish_description(kinds: &KindRegistry) -> String {
    let mut out = String::from(
        "Publish a tool of a registered kind under this tenant's namespace. \
         Minimal example spec per kind:",
    );
    for name in kinds.names() {
        let Some(kind) = kinds.get(name) else {
            continue;
        };
        let example = kind.example();
        let spec_json = serde_json::to_string(&example.spec).unwrap_or_default();
        out.push_str(&format!(" {name} -- spec: {spec_json}. {}", example.blurb));
    }
    out.push_str(
        " Name must match ^[a-z][a-z0-9_]{1,40}$. A rejection names the failing field, \
         what was expected, and a corrected example -- fix it and resubmit. Try \
         `host.tool_test` on a published tool before a real call, or call \
         `host.quickstart(kind)` for a filled-in worked example.",
    );
    out
}

fn host_tools(kinds: &KindRegistry) -> Vec<Tool> {
    vec![
        Tool::new(
            "host.whoami",
            "Return the calling tenant's identity.",
            host_schema(json!({}), &[]),
        ),
        Tool::new(
            "host.tool_publish",
            tool_publish_description(kinds),
            host_schema(
                json!({
                    "name": {"type": "string"},
                    "kind": {"type": "string"},
                    "spec": {"type": "object"},
                }),
                &["name", "kind", "spec"],
            ),
        ),
        Tool::new(
            "host.quickstart",
            "Return the shortest ordered sequence of calls to a working tool of `kind`, \
             with your namespace and a filled-in example already substituted in, plus the \
             current limits. Read-only. Call this before host.tool_publish if you're not \
             sure what a spec should look like. Unauthenticated callers get the signup \
             step first.",
            host_schema(json!({"kind": {"type": "string"}}), &["kind"]),
        ),
        Tool::new(
            "host.tool_list",
            "List this tenant's published tools.",
            host_schema(json!({}), &[]),
        ),
        Tool::new(
            "host.tool_remove",
            "Remove a published tool by its local name.",
            host_schema(json!({"name": {"type": "string"}}), &["name"]),
        ),
        Tool::new(
            "host.tool_logs",
            "Return the most recent log lines for one of this tenant's tools.",
            host_schema(
                json!({"name": {"type": "string"}, "limit": {"type": "integer"}}),
                &["name"],
            ),
        ),
        Tool::new(
            "host.tool_test",
            "Dry-run a published tool: performs the real call but records no `calls` row \
             and echoes the rendered request back with secrets redacted, for debugging a spec.",
            host_schema(
                json!({"name": {"type": "string"}, "args": {"type": "object"}}),
                &["name", "args"],
            ),
        ),
        Tool::new(
            "host.tool_call",
            "Invoke a tool this tenant has already published, by its local name -- the \
             same real, metered call as calling it directly by its namespaced name \
             (<namespace>.<name>), for a session that has no way to see its own \
             namespaced tool name yet. Unlike host.tool_test, this counts toward \
             host.usage and appears in host.tool_logs.",
            host_schema(
                json!({"name": {"type": "string"}, "args": {"type": "object"}}),
                &["name", "args"],
            ),
        ),
        Tool::new(
            "host.usage",
            "Calls, errors and duration percentiles for this tenant over a window.",
            host_schema(json!({"window": {"type": "string"}}), &[]),
        ),
        Tool::new(
            "host.secret_set",
            "Store an encrypted secret value under this tenant's namespace.",
            host_schema(
                json!({"name": {"type": "string"}, "value": {"type": "string"}}),
                &["name", "value"],
            ),
        ),
        Tool::new(
            "host.secret_list",
            "List this tenant's secret names (never their values).",
            host_schema(json!({}), &[]),
        ),
        Tool::new(
            "host.registry_publish",
            "Publish this tenant's server.json to the configured MCP registry \
             (requires --registry-url and admin.tenant_verify_namespace first).",
            host_schema(json!({}), &[]),
        ),
    ]
}

fn admin_tools() -> Vec<Tool> {
    vec![
        Tool::new(
            "admin.tenants",
            "List every tenant.",
            schema(json!({}), &[]),
        ),
        Tool::new(
            "admin.tenant_disable",
            "Disable a tenant; its key stops authenticating.",
            schema(json!({"tenant": {"type": "string"}}), &["tenant"]),
        ),
        Tool::new(
            "admin.tenant_enable",
            "Re-enable a disabled tenant.",
            schema(json!({"tenant": {"type": "string"}}), &["tenant"]),
        ),
        Tool::new(
            "admin.usage",
            "Calls, errors and duration percentiles for every tenant and tool over a window.",
            schema(json!({"window": {"type": "string"}}), &[]),
        ),
        Tool::new(
            "admin.tool_list",
            "List a specific tenant's published tools.",
            schema(json!({"tenant": {"type": "string"}}), &["tenant"]),
        ),
        Tool::new(
            "admin.tenant_verify_namespace",
            "Mark a tenant's domain namespace verified for registry publishing \
             (the verification METHOD is operator-determined, out of scope here).",
            schema(
                json!({
                    "tenant": {"type": "string"},
                    "domain_namespace": {"type": "string"},
                }),
                &["tenant", "domain_namespace"],
            ),
        ),
    ]
}

// ---- secret resolver / call log, bridging async DB to the sync Kind API --

struct MapSecretResolver(HashMap<String, String>);
impl SecretResolver for MapSecretResolver {
    fn resolve(&self, name: &str) -> Option<String> {
        self.0.get(name).cloned()
    }
}

async fn build_secret_resolver(
    state: &AppState,
    tenant_id: i64,
) -> Result<Arc<dyn SecretResolver>, AppError> {
    let names = state.db.list_secret_names(tenant_id).await?;
    let mut map = HashMap::with_capacity(names.len());
    for name in names {
        if let Some((ct, nonce)) = state.db.get_secret(tenant_id, name.clone()).await? {
            let plain = state.secrets.decrypt(&ct, &nonce)?;
            map.insert(name, plain);
        }
    }
    Ok(Arc::new(MapSecretResolver(map)))
}

struct BufferedLog(std::sync::Mutex<Vec<String>>);
impl CallLog for BufferedLog {
    fn log(&self, line: &str) {
        if let Ok(mut guard) = self.0.lock() {
            guard.push(line.to_string());
        }
    }
}

/// See `kinds::ResourceSink`: a single-slot cell a `Kind::call` (currently
/// only `python`) writes its sandboxed subprocess's CPU/memory usage into,
/// read back after the call to fill in the `calls` row (PRD-mcphost-code-tools
/// requirement 8). `None` (the default) means the kind never reported any --
/// exactly what happens for `echo`/`http`.
struct CellResourceSink(std::sync::Mutex<Option<(i64, i64)>>);
impl ResourceSink for CellResourceSink {
    fn record(&self, cpu_ms: i64, peak_rss_kb: i64) {
        if let Ok(mut guard) = self.0.lock() {
            *guard = Some((cpu_ms, peak_rss_kb));
        }
    }
}

// ---- the handler -----------------------------------------------------

#[derive(Clone)]
pub struct McpHostHandler {
    pub state: Arc<AppState>,
}

impl McpHostHandler {
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    async fn dispatch_tenant_tool(
        &self,
        tenant: &Tenant,
        name: &str,
        args: Value,
    ) -> Result<Value, AppError> {
        match name {
            "host.whoami" => Ok(control::whoami(tenant)),
            "host.tool_publish" => control::tool_publish(&self.state, tenant, &args).await,
            "host.tool_list" => control::tool_list(&self.state, tenant).await,
            "host.tool_remove" => control::tool_remove(&self.state, tenant, &args).await,
            "host.tool_logs" => control::tool_logs(&self.state, tenant, &args).await,
            "host.tool_test" => self.tool_test(tenant, args).await,
            "host.tool_call" => self.host_tool_call(tenant, args).await,
            "host.usage" => control::usage(&self.state, tenant, &args).await,
            "host.secret_set" => control::secret_set(&self.state, tenant, &args).await,
            "host.secret_list" => control::secret_list(&self.state, tenant).await,
            "host.registry_publish" => control::registry_publish(&self.state, tenant, &args).await,
            other => Err(AppError::ToolNotFound(other.to_string())),
        }
    }

    async fn dispatch_admin_tool(&self, name: &str, args: Value) -> Result<Value, AppError> {
        match name {
            "admin.tenants" => admin::tenants(&self.state).await,
            "admin.tenant_disable" => admin::tenant_disable(&self.state, &args).await,
            "admin.tenant_enable" => admin::tenant_enable(&self.state, &args).await,
            "admin.usage" => admin::usage(&self.state, &args).await,
            "admin.tool_list" => admin::tool_list(&self.state, &args).await,
            "admin.tenant_verify_namespace" => {
                admin::tenant_verify_namespace(&self.state, &args).await
            }
            other => Err(AppError::ToolNotFound(other.to_string())),
        }
    }

    /// Execute a published tenant tool (`<namespace>.<local-name>`),
    /// metering the call and enforcing the 30s deadline.
    async fn call_published_tool(
        &self,
        tenant: &Tenant,
        local_name: &str,
        args: Value,
        mcp_name_mismatch: bool,
    ) -> Result<Value, AppError> {
        let row: ToolRow = self
            .state
            .db
            .get_tool(tenant.id, local_name.to_string())
            .await?
            .ok_or_else(|| AppError::ToolNotFound(local_name.to_string()))?;
        let kind: Arc<dyn Kind> = self.state.kinds.get(&row.kind).ok_or_else(|| {
            AppError::Internal(format!(
                "published tool names unregistered kind '{}'",
                row.kind
            ))
        })?;

        let descriptor = kind.describe(&row.spec);
        if let Ok(validator) = jsonschema::validator_for(&descriptor.input_schema)
            && let Err(e) = validator.validate(&args)
        {
            // AC4: the wire code for a schema-invalid call to a *published*
            // tool is `args_invalid`, distinct from `AppError::InvalidArgs`
            // (used for missing control-plane arguments elsewhere).
            return Err(AppError::ArgsInvalid(e.to_string()));
        }

        let secrets = build_secret_resolver(&self.state, tenant.id).await?;
        let log = Arc::new(BufferedLog(std::sync::Mutex::new(Vec::new())));
        let resources = Arc::new(CellResourceSink(std::sync::Mutex::new(None)));
        let ctx = CallCtx {
            tenant_id: tenant.id,
            namespace: tenant.namespace.clone(),
            secrets,
            deadline: Instant::now() + self.state.call_timeout,
            log: log.clone() as Arc<dyn CallLog>,
            test_mode: false,
            resources: resources.clone() as Arc<dyn ResourceSink>,
        };

        let start = Instant::now();
        if mcp_name_mismatch {
            tracing::warn!(tenant = %tenant.namespace, tool = %local_name, "Mcp-Name header does not match call body's tool name");
        }
        let outcome =
            tokio::time::timeout(self.state.call_timeout, kind.call(&row.spec, args, &ctx)).await;
        let duration_ms = start.elapsed().as_millis() as i64;
        let (cpu_ms, peak_rss_kb) = resources.0.lock().map(|g| *g).unwrap_or_default().unzip();

        for line in log.0.lock().map(|g| g.clone()).unwrap_or_default() {
            let _ = self
                .state
                .db
                .append_log(tenant.id, local_name.to_string(), line)
                .await;
        }

        match outcome {
            Ok(Ok(value)) => {
                // Requirement 8: every `tools/call` writes a `calls` row.
                // If that write fails (AC14: an unwritable database), the
                // call must not silently succeed with unmetered usage —
                // report the storage failure even though the `Kind::call`
                // itself completed.
                if let Err(storage_err) = self
                    .state
                    .db
                    .record_call(
                        tenant.id,
                        local_name.to_string(),
                        duration_ms,
                        true,
                        None,
                        cpu_ms,
                        peak_rss_kb,
                    )
                    .await
                {
                    tracing::error!(
                        tenant = %tenant.namespace, method = "tools/call", tool = %local_name,
                        duration_ms, status = "error", error_class = "storage", mcp_name_mismatch,
                    );
                    return Err(storage_err);
                }
                tracing::info!(
                    tenant = %tenant.namespace, method = "tools/call", tool = %local_name,
                    duration_ms, status = "ok", mcp_name_mismatch,
                );
                Ok(value)
            }
            Ok(Err(kind_err)) => {
                let app_err = AppError::from(kind_err);
                let _ = self
                    .state
                    .db
                    .record_call(
                        tenant.id,
                        local_name.to_string(),
                        duration_ms,
                        false,
                        Some(app_err.code().to_string()),
                        cpu_ms,
                        peak_rss_kb,
                    )
                    .await;
                tracing::info!(
                    tenant = %tenant.namespace, method = "tools/call", tool = %local_name,
                    duration_ms, status = "error", error_class = app_err.code(), mcp_name_mismatch,
                );
                Err(app_err)
            }
            Err(_elapsed) => {
                let _ = self
                    .state
                    .db
                    .record_call(
                        tenant.id,
                        local_name.to_string(),
                        duration_ms,
                        false,
                        Some("call_timeout".to_string()),
                        cpu_ms,
                        peak_rss_kb,
                    )
                    .await;
                tracing::info!(
                    tenant = %tenant.namespace, method = "tools/call", tool = %local_name,
                    duration_ms, status = "error", error_class = "call_timeout", mcp_name_mismatch,
                );
                Err(AppError::CallTimeout)
            }
        }
    }

    /// `host.tool_test` (P1 requirement 9, AC12): the same real call as
    /// `call_published_tool`, minus the `calls` row (the resolved open
    /// question in the PRD is decided here: a test call DOES count toward
    /// the outbound rate limit, since it hits the real upstream and
    /// consumes the same quota) and minus persisted `logs` lines (this is
    /// an out-of-band debug call, not part of the tool's operational
    /// history). `ctx.test_mode = true` tells a `Kind` that supports it
    /// (`http`) to echo the rendered request, secrets redacted, alongside
    /// the response.
    async fn tool_test(&self, tenant: &Tenant, args: Value) -> Result<Value, AppError> {
        let local_name = args
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::InvalidArgs("missing required argument 'name'".into()))?
            .to_string();
        let call_args = args
            .get("args")
            .cloned()
            .unwrap_or_else(|| Value::Object(Default::default()));

        let row: ToolRow = self
            .state
            .db
            .get_tool(tenant.id, local_name.clone())
            .await?
            .ok_or_else(|| AppError::ToolNotFound(local_name.clone()))?;
        let kind: Arc<dyn Kind> = self.state.kinds.get(&row.kind).ok_or_else(|| {
            AppError::Internal(format!(
                "published tool names unregistered kind '{}'",
                row.kind
            ))
        })?;

        let descriptor = kind.describe(&row.spec);
        if let Ok(validator) = jsonschema::validator_for(&descriptor.input_schema)
            && let Err(e) = validator.validate(&call_args)
        {
            return Err(AppError::ArgsInvalid(e.to_string()));
        }

        let secrets = build_secret_resolver(&self.state, tenant.id).await?;
        let ctx = CallCtx {
            tenant_id: tenant.id,
            namespace: tenant.namespace.clone(),
            secrets,
            deadline: Instant::now() + self.state.call_timeout,
            log: Arc::new(NullLog) as Arc<dyn CallLog>,
            test_mode: true,
            resources: Arc::new(NullResourceSink),
        };

        match tokio::time::timeout(
            self.state.call_timeout,
            kind.call(&row.spec, call_args, &ctx),
        )
        .await
        {
            // AC19: `tenant_key` may appear anywhere inside the echoed
            // request `call_tool` already redacted from `args` by key name
            // before it reached here -- this final pass catches the same
            // key name inside whatever the kind's own (by-value) redaction
            // returned, proving the redaction is by key name rather than by
            // matching a value the caller controls.
            Ok(Ok(value)) => Ok(crate::secrets::redact_keys(&value, &["tenant_key"])),
            Ok(Err(kind_err)) => Err(AppError::from(kind_err)),
            Err(_elapsed) => Err(AppError::CallTimeout),
        }
    }

    /// `host.tool_call` (PRD-mcphost-session-key requirements 11-15,
    /// AC11-15): resolves `name` against the caller's own namespace and
    /// delegates outright to `call_published_tool` -- the same
    /// metering, schema-validation and secret-resolution path a direct
    /// `<namespace>.<name>` call takes. Requirement 12's whole point is
    /// that a `host.tool_call` invocation must be indistinguishable from a
    /// direct call in `host.usage` and `host.tool_logs`, unlike
    /// `host.tool_test`, which deliberately records neither -- so this
    /// must never reimplement any of that logic, only reach it.
    async fn host_tool_call(&self, tenant: &Tenant, args: Value) -> Result<Value, AppError> {
        let local_name = args
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::InvalidArgs("missing required argument 'name'".into()))?
            .to_string();
        let call_args = args
            .get("args")
            .cloned()
            .unwrap_or_else(|| Value::Object(Default::default()));
        // AC15: `call_published_tool`'s `get_tool` lookup is already scoped
        // to `tenant.id`, so a name only some other tenant published simply
        // isn't found here -- `ToolNotFound`, with nothing in the error to
        // distinguish "never published by anyone" from "published by
        // someone else".
        self.call_published_tool(tenant, &local_name, call_args, false)
            .await
    }
}

impl ServerHandler for McpHostHandler {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "Call `signup` with a display name to receive a bearer key. The `host.*` \
                 control plane -- including `host.tool_publish` and `host.tool_call` -- is \
                 already visible in this tools/list, before you have a key. Pass the key \
                 `signup` returns as the `tenant_key` argument on every call after that; no \
                 reconnect and no Authorization header is required.",
        )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let parts = get_parts(&ctx)?;
        let auth = resolve_auth(&self.state, parts)
            .await
            .map_err(AppError::into_error_data)?;

        // PRD-mcphost-protocol-compat requirement 5: every branch resolves
        // to a (tools, ttl_ms) pair here, and the cache fields are applied
        // exactly once below, after the match. A branch-by-branch chain of
        // `.with_ttl_ms()`/`.with_cache_scope()` calls would satisfy today's
        // tests but silently drop the fields the next time an `Auth`
        // variant is added -- see the anonymous/admin branches this
        // replaced, which did exactly that.
        let (tools, ttl_ms) = match auth {
            // PRD-mcphost-session-key requirement 1 / AC1-3: the `host.*`
            // control plane (and `host.tool_call`) is discoverable before
            // signup -- an anonymous or invalid-bearer caller cannot attach
            // a key mid-session any other way (that's this PRD's whole
            // premise), so it must already be able to see, and read the
            // schema of, every tool it will need. These are static
            // descriptors carrying no tenant data; no namespaced tool, no
            // `admin.*` tool and no tenant-existence information is ever in
            // this branch.
            Auth::Anonymous | Auth::Invalid => {
                let mut tools = vec![signup_tool()];
                tools.extend(host_tools(&self.state.kinds));
                (tools, TOOLS_LIST_TTL_MS_STEADY)
            }
            Auth::Admin => (admin_tools(), TOOLS_LIST_TTL_MS_STEADY),
            Auth::Tenant(tenant) => {
                let mut tools = host_tools(&self.state.kinds);
                let rows = self
                    .state
                    .db
                    .list_tools(tenant.id)
                    .await
                    .map_err(AppError::into_error_data)?;
                for row in rows {
                    if let Some(kind) = self.state.kinds.get(&row.kind) {
                        let descriptor = kind.describe(&row.spec);
                        tools.push(Tool::new(
                            format!("{}.{}", tenant.namespace, row.name),
                            descriptor.description,
                            value_to_json_object(descriptor.input_schema),
                        ));
                    }
                }
                // AC18 / PRD requirement 14: ttlMs must go to 0 for the 60s
                // after a publish OR a remove. `tenant.last_tool_change_unix`
                // (stamped by both Db::upsert_tool and Db::remove_tool) is
                // used rather than max(created_at) over the *surviving*
                // tool rows, because a remove deletes exactly the row that
                // reading would need -- the tenant's only tool being removed
                // used to (wrongly) read back as "no recent change".
                let recent_change = tenant.last_tool_change_unix > 0
                    && now_unix() - tenant.last_tool_change_unix < TOOLS_LIST_TTL_GRACE_SECS as i64;
                let ttl_ms = if recent_change {
                    0
                } else {
                    TOOLS_LIST_TTL_MS_STEADY
                };
                (tools, ttl_ms)
            }
        };
        Ok(ListToolsResult::with_all_items(tools)
            .with_ttl_ms(ttl_ms)
            .with_cache_scope(CacheScope::Private))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let parts = get_parts(&ctx)?;
        let source = source_ip(parts);
        let header_name = mcp_name_header(parts);
        let body_name = request.name.to_string();
        let mismatch = header_name.is_some_and(|h| h != body_name);

        let mut auth = resolve_auth(&self.state, parts)
            .await
            .map_err(AppError::into_error_data)?;

        let raw_args = Value::Object(request.arguments.unwrap_or_default());
        // Requirement 5/6: the header always wins whenever the connection
        // sent one at all, valid or not -- the tenant_key argument is only
        // ever consulted when `resolve_auth` found no `Authorization`
        // header to resolve in the first place. Requirement 9: this is
        // `call_tool` only, never `list_tools`, which never reaches here.
        if matches!(auth, Auth::Anonymous) {
            auth = resolve_tenant_key_auth(&self.state, &raw_args)
                .await
                .map_err(AppError::into_error_data)?;
        }
        // Requirements 16/17: redacted by key name, recursively, exactly
        // once here, so every dispatch branch below -- a `host.*` tool, an
        // `admin.*` tool, or a direct namespaced call -- works from
        // arguments that can no longer carry the key, at any depth (AC19),
        // rather than each callee having to remember to do it itself.
        let args = crate::secrets::redact_keys(&raw_args, &["tenant_key"]);

        let outcome: Result<Value, AppError> = match (&auth, body_name.as_str()) {
            (_, "signup") => control::signup(&self.state, &args, &source).await,
            // Requirement 4 / AC3-4: `host.quickstart` is readable before
            // signup, same as the rest of the `host.*` control plane in
            // `list_tools` -- an authenticated tenant gets its own
            // namespace filled in; anyone else gets the signup step, no
            // tenant data. Deliberately checked before the blanket
            // Anonymous/Invalid -> Unauthorized arm below, the same way
            // `signup` itself is.
            (Auth::Tenant(tenant), "host.quickstart") => {
                control::quickstart(&self.state, Some(tenant), &args)
            }
            (_, "host.quickstart") => control::quickstart(&self.state, None, &args),
            (Auth::Anonymous | Auth::Invalid, _) => Err(AppError::Unauthorized),
            (Auth::Admin, name) if name.starts_with("admin.") => {
                self.dispatch_admin_tool(name, args).await
            }
            (Auth::Admin, _) => Err(AppError::Forbidden),
            (Auth::Tenant(_), name) if name.starts_with("admin.") => {
                let _ = name;
                Err(AppError::Forbidden)
            }
            (Auth::Tenant(tenant), name) if name.starts_with("host.") => {
                self.dispatch_tenant_tool(tenant, name, args).await
            }
            (Auth::Tenant(tenant), name) => match name.split_once('.') {
                Some((ns, local)) if ns == tenant.namespace => {
                    self.call_published_tool(tenant, local, args, mismatch)
                        .await
                }
                _ => Err(AppError::ToolNotFound(name.to_string())),
            },
        };

        match outcome {
            Ok(value) => Ok(CallToolResponse::from(CallToolResult::structured(value))),
            Err(app_err) => Err(app_err.into_error_data()),
        }
    }

    async fn on_initialized(&self, _context: NotificationContext<RoleServer>) {}
}
