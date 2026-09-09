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
    CallCtx, CallLog, Kind, KindRegistry, MAX_TEST_INVOCATIONS, NullLog, NullResourceSink,
    ResourceSink, SecretResolver, describe_args_error, run_spec_test,
};
use crate::state::{
    AppState, MAX_SPEC_BYTES, TOOLS_LIST_TTL_GRACE_SECS, TOOLS_LIST_TTL_MS_STEADY, now_unix,
};
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
    /// PRD-grand-loop-billing: `Tenant` grew three billing columns
    /// (`plan`/`plan_since`/`billing_ref`), pushing this enum over
    /// clippy's `large_enum_variant` threshold when carried by value
    /// alongside the zero-sized `Anonymous`/`Invalid`/`Admin` variants --
    /// boxed per the lint's own suggestion rather than shrinking `Tenant`
    /// itself (every other variant is unaffected; every call site already
    /// takes `&Tenant`, which `Box<Tenant>` derefs to for free).
    Tenant(Box<Tenant>),
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
        Some(t) => Ok(Auth::Tenant(Box::new(t))),
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
        Some(t) => Ok(Auth::Tenant(Box::new(t))),
        None => Ok(Auth::Invalid),
    }
}

fn get_parts(ctx: &RequestContext<RoleServer>) -> Result<&http::request::Parts, McpError> {
    ctx.extensions.get::<http::request::Parts>().ok_or_else(|| {
        AppError::Internal("no HTTP request parts on this call".into()).into_error_data()
    })
}

/// The source address used for signup rate limiting, `signup_events`, and
/// every other per-source read (requirement 4: one call site, funneling
/// through [`crate::state::resolve_source_ip`]). PRD-mcphost-client-ip-behind-proxy:
/// mcphost.dev runs behind Caddy on the same box, so the TCP peer axum's
/// `ConnectInfo` extractor attaches is loopback for every proxied request --
/// `resolve_source_ip` reads `X-Forwarded-For` in that case and falls back
/// to the peer address otherwise (or when the header is missing or
/// unparseable). HTTP header lookup is case-insensitive, so
/// `X-Forwarded-For` (Caddy's default casing) matches the lowercase name
/// here.
fn source_ip(parts: &http::request::Parts) -> String {
    let peer = parts
        .extensions
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|ci| ci.0.ip().to_string());
    let forwarded_for = parts
        .headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok());
    crate::state::resolve_source_ip(peer.as_deref(), forwarded_for)
}

fn mcp_name_header(parts: &http::request::Parts) -> Option<String> {
    parts
        .headers
        .get("Mcp-Name")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

/// PRD-mcphost-synthetic-flag requirement 2: the raw `x-mcphost-synthetic`
/// header value, unvalidated -- `control::signup` (via
/// `validate_synthetic_header`) is where the charset/length check and the
/// "invalid logs a warning" behavior live, same division of labor as
/// `source_ip`/`mcp_name_header` above (HTTP-layer extraction here,
/// business-logic validation in `control.rs`).
fn synthetic_header(parts: &http::request::Parts) -> Option<String> {
    parts
        .headers
        .get("x-mcphost-synthetic")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

/// PRD-mcphost-tenant-attribution requirement 2: `signup_events.user_agent`,
/// when the transport exposes one -- best-effort, same as `mcp_name_header`
/// above (an absent or non-UTF-8 header is silently `None`, never an
/// error).
fn user_agent_header(parts: &http::request::Parts) -> Option<String> {
    parts
        .headers
        .get(http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

/// PRD-mcphost-tenant-attribution requirement 2: the caller's
/// `clientInfo.name`/`version`. `RequestContext::client_info` (`rmcp`'s own
/// SEP-2575-aware accessor) is deliberately used instead of reading
/// `ctx.peer.peer_info()` directly: this host's streamable-HTTP config
/// (`with_legacy_session_mode(false)`) runs every `tools/call` as its own
/// stateless request rather than a session `initialize` persists across
/// (this crate's own `tests/common`'s `_meta` injection on every call, not
/// just `initialize`, is the other side of the same fact) -- `client_info`
/// reads the per-request `_meta["io.modelcontextprotocol/clientInfo"]` a
/// SEP-2575 client sends on that call in that case, falling back to the
/// session-persisted value only for a genuinely stateful/legacy peer.
/// `None` when neither is present (a client that doesn't send `clientInfo`
/// at all -- signup still succeeds, requirement 2 doesn't make this
/// mandatory).
fn peer_client_info(ctx: &RequestContext<RoleServer>) -> Option<(String, String)> {
    ctx.client_info()
        .map(|info| (info.name.clone(), info.version.clone()))
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
        // PRD-mcphost-sandbox-ready P1 requirement 7 (AC8): a client that
        // reads this description before publishing must see, right here,
        // that this kind is currently rejected -- so it never sends the
        // doomed call. `sandbox_status()` is `None` for every kind with no
        // sandbox concept (`echo`, `http`), so this is a no-op for them.
        if let Some(status) = kind.sandbox_status()
            && !status.ready
        {
            out.push_str(&format!(
                " NOTE: {name}-kind publishes are currently rejected on this host with \
                 sandbox_unavailable ({}).",
                status.detail
            ));
        }
    }
    out.push_str(
        " Name must match ^[a-z][a-z0-9_]{1,40}$. A rejection names the failing field, \
         what was expected, and a corrected example -- fix it and resubmit. Try \
         `host.tool_test` on a published tool before a real call, or call \
         `host.quickstart(kind)` for a filled-in worked example.",
    );
    out
}

/// PRD-mcphost-tool-test AC9: `host.spec_test`, unlike every other `host.*`
/// descriptor, must be absent from `tools/list` for an anonymous/invalid
/// caller (present, and callable, only once authenticated) -- `authenticated`
/// gates pushing it onto the returned vec; every other `host.*` tool is
/// unaffected and stays visible pre-auth (PRD-mcphost-session-key).
fn host_tools(kinds: &KindRegistry, authenticated: bool) -> Vec<Tool> {
    let mut tools = vec![
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
            "host.bridge_test",
            "Dry-run an `http`-kind spec (typically a declarative REST-bridge `upstream` \
             spec) against its real upstream without publishing it: no tool is created, no \
             `calls` row is written, and the rendered request is echoed back with secrets \
             redacted, same as host.tool_test but for a spec you haven't published yet. An \
             invalid spec reports the same failure class host.tool_publish would.",
            host_schema(
                json!({"spec": {"type": "object"}, "args": {"type": "object"}}),
                &["spec", "args"],
            ),
        ),
        Tool::new(
            "host.tool_run",
            "Debug run of a published tool: the same sandbox and limits as a real call, but \
             returns full stdout and stderr (each capped at 64 KiB) and the exit code alongside \
             the result, and records no `calls` row and no metering. Only kinds with a notion of \
             a subprocess (`python`) support this; other kinds return `tool_run_unsupported`. \
             Rate-limited to 30 calls per tenant per minute, independent of `host.usage`.",
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
        Tool::new(
            "billing.plans",
            "The plan catalog (price and quotas per plan) and whether Stripe billing is \
             configured on this host. Anonymous callers get the same answer as tenants.",
            host_schema(json!({}), &[]),
        ),
        Tool::new(
            "billing.status",
            "This tenant's plan, usage against each quota, and when the daily call quota \
             resets.",
            host_schema(json!({}), &[]),
        ),
        Tool::new(
            "billing.checkout",
            "Create (or reuse an open one for the same plan) a Stripe Checkout URL to \
             upgrade this tenant, defaulting to the pro plan. Returns billing_unavailable \
             if this host has no Stripe key configured -- call billing.plans first to check.",
            host_schema(json!({"plan": {"type": "string"}}), &[]),
        ),
    ];
    if authenticated {
        tools.push(Tool::new(
            "host.spec_test",
            "Dry-run a tool spec before it is ever published: validates it, then runs up \
             to 5 example invocations through the same sandbox and limits a published call \
             uses, returning each invocation's ok/output/duration_ms (or a bounded exception \
             on failure) plus the args_schema and requirements a publish of this spec would \
             infer. No tool row is ever written. Distinct from host.tool_test, which dry-runs \
             an already-published tool by name.",
            host_schema(
                json!({
                    "kind": {"type": "string"},
                    "spec": {"type": "object"},
                    "invocations": {
                        "type": "array",
                        "items": {"type": "object"},
                        "maxItems": MAX_TEST_INVOCATIONS,
                    },
                }),
                &["kind", "spec", "invocations"],
            ),
        ));
    }
    tools
}

fn admin_tools() -> Vec<Tool> {
    vec![
        Tool::new(
            "admin.tenants",
            "List every tenant, optionally filtered to display names starting with `prefix` \
             and/or by `synthetic` (`true`: only labeled tenants, `false`: only unlabeled, \
             `all`: no filter -- the default). Every row carries a `synthetic` field, null \
             for unlabeled.",
            schema(
                json!({
                    "prefix": {"type": "string"},
                    "synthetic": {"type": "string", "enum": ["true", "false", "all"]},
                }),
                &[],
            ),
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
            "admin.tenant_delete",
            "Delete a tenant and, in the same transaction, cascade-remove every row it \
             owns (tools, secrets, calls, logs, registry document).",
            schema(json!({"tenant": {"type": "string"}}), &["tenant"]),
        ),
        Tool::new(
            "admin.tenant_delete_by_prefix",
            "Cascade-delete (or, by default, dry-run preview) every tenant whose display \
             name starts with `prefix` (at least 4 characters), up to 500 per call.",
            schema(
                json!({
                    "prefix": {"type": "string"},
                    "dry_run": {"type": "boolean"},
                }),
                &["prefix"],
            ),
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
        Tool::new(
            "admin.sandbox_recheck",
            "Re-run the sandbox self-test immediately (rather than waiting for the periodic \
             recheck) and return the fresh SandboxStatus -- call after fixing whatever made \
             sandbox_ready false on /healthz, to confirm without restarting the unit.",
            schema(json!({}), &[]),
        ),
        Tool::new(
            "admin.billing_ledger",
            "Every ledgered billing event (webhook or admin.plan_set), newest first, capped \
             at 1000, optionally filtered by since/until (unix seconds) and tenant.",
            schema(
                json!({
                    "since": {"type": "integer"},
                    "until": {"type": "integer"},
                    "tenant": {"type": "string"},
                }),
                &[],
            ),
        ),
        Tool::new(
            "admin.plan_set",
            "Support override: set a tenant's plan directly, ledgered as event_type \
             admin.plan_set.",
            schema(
                json!({
                    "tenant": {"type": "string"},
                    "plan": {"type": "string"},
                    "reason": {"type": "string"},
                }),
                &["tenant", "plan"],
            ),
        ),
        Tool::new(
            "admin.meter_status",
            "PRD-mcphost-metered-overage P1 AC11: the last ledgered `mcphost billing emit-meter` \
             batch's span, the current meter_lag, and per-tenant emitted call counts for the \
             current UTC month.",
            schema(json!({}), &[]),
        ),
        Tool::new(
            "admin.tenant_set_synthetic",
            "Set or clear (`label: null`) one tenant's `synthetic` label. `label`, when a \
             string, must match ^[a-z0-9][a-z0-9:_-]{0,63}$.",
            schema(
                json!({
                    "tenant": {"type": "string"},
                    "label": {"type": ["string", "null"]},
                }),
                &["tenant", "label"],
            ),
        ),
        Tool::new(
            "admin.tenants_set_synthetic",
            "Bulk retro-tag: apply `label` to every tenant whose display name matches the \
             SQL LIKE pattern `name_like` (e.g. `%Chen%`). `dry_run` (default true) only \
             returns the matched tenants and count; `dry_run: false` applies the label and \
             returns the same list.",
            schema(
                json!({
                    "name_like": {"type": "string"},
                    "label": {"type": "string"},
                    "dry_run": {"type": "boolean"},
                }),
                &["name_like", "label"],
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
            "host.bridge_test" => self.bridge_test(tenant, args).await,
            "host.spec_test" => self.spec_test(tenant, args).await,
            "host.tool_run" => self.tool_run(tenant, args).await,
            "host.tool_call" => self.host_tool_call(tenant, args).await,
            "host.usage" => control::usage(&self.state, tenant, &args).await,
            "host.secret_set" => control::secret_set(&self.state, tenant, &args).await,
            "host.secret_list" => control::secret_list(&self.state, tenant).await,
            "host.registry_publish" => control::registry_publish(&self.state, tenant, &args).await,
            "billing.status" => crate::billing::status(&self.state, tenant).await,
            "billing.checkout" => crate::billing::checkout(&self.state, tenant, &args).await,
            other => Err(AppError::ToolNotFound(other.to_string())),
        }
    }

    async fn dispatch_admin_tool(&self, name: &str, args: Value) -> Result<Value, AppError> {
        match name {
            "admin.tenants" => admin::tenants(&self.state, &args).await,
            "admin.tenant_disable" => admin::tenant_disable(&self.state, &args).await,
            "admin.tenant_enable" => admin::tenant_enable(&self.state, &args).await,
            "admin.tenant_delete" => admin::tenant_delete(&self.state, &args).await,
            "admin.tenant_delete_by_prefix" => {
                admin::tenant_delete_by_prefix(&self.state, &args).await
            }
            "admin.usage" => admin::usage(&self.state, &args).await,
            "admin.tool_list" => admin::tool_list(&self.state, &args).await,
            "admin.tenant_verify_namespace" => {
                admin::tenant_verify_namespace(&self.state, &args).await
            }
            "admin.sandbox_recheck" => admin::sandbox_recheck(&self.state).await,
            "admin.billing_ledger" => admin::billing_ledger(&self.state, &args).await,
            "admin.plan_set" => admin::plan_set(&self.state, &args).await,
            "admin.meter_status" => admin::meter_status(&self.state).await,
            "admin.tenant_set_synthetic" => admin::tenant_set_synthetic(&self.state, &args).await,
            "admin.tenants_set_synthetic" => {
                admin::tenants_set_synthetic(&self.state, &args).await
            }
            other => Err(AppError::ToolNotFound(other.to_string())),
        }
    }

    /// PRD-grand-loop-billing AC3: reject with `quota_exceeded` when
    /// `tenant` has already made `plan.calls_per_day` (or more) successful
    /// calls since the most recent UTC midnight. Shared by every path that
    /// reaches [`Self::call_published_tool`] (a direct namespaced call and
    /// `host.tool_call` both go through it); `host.tool_test`/`host.tool_run`
    /// deliberately do not call this -- neither writes a `calls` row or
    /// counts toward this quota (same distinction `host.usage` already
    /// draws).
    async fn check_calls_quota(&self, tenant: &Tenant) -> Result<(), AppError> {
        let plan = self.state.plans.get(&tenant.plan).ok_or_else(|| {
            AppError::Internal(format!(
                "tenant's plan '{}' is not in the loaded plan catalog",
                tenant.plan
            ))
        })?;
        let midnight = crate::state::utc_midnight_unix(crate::state::now_unix());
        let used = self.state.db.count_calls_since(tenant.id, midnight, true).await?;
        if used >= plan.calls_per_day {
            let resets_at = crate::state::rfc3339_from_unix(midnight + 86_400);
            return Err(crate::billing::quota_exceeded(
                &tenant.plan,
                "calls_per_day",
                plan.calls_per_day,
                used,
                Some(resets_at),
            ));
        }
        Ok(())
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
            // Requirement 1/AC3 (PRD-mcphost-python-kind-runtime): phase
            // `args_coercion`, naming the argument and both types -- shared
            // with every kind via `describe_args_error`, wired through
            // `AppError::Structured` (not `ArgsInvalid`) so the extra `data`
            // fields survive into the JSON-RPC response; the wire code stays
            // pinned to exactly `args_invalid` either way.
            let data = describe_args_error(&e);
            return Err(AppError::Structured {
                code: "args_invalid",
                message: e.to_string(),
                data,
            });
        }

        // PRD-grand-loop-billing AC3: `calls_per_day` enforcement, checked
        // before the call ever dispatches (same "rejected before any
        // sandboxed work happens" shape as the sandbox_unavailable check
        // elsewhere in this file) -- a rejected call writes no `calls` row
        // at all, a stronger guarantee than "no row with ok = 1".
        self.check_calls_quota(tenant).await?;

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
            tool_name: Some(local_name.to_string()),
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
                // PRD-mcphost-first-call-reliability requirement 6 (P1,
                // AC6): a call that waited (bounded) for a building
                // environment, or came back past the bound as the
                // structured `building` result, is metered distinctly from
                // an ordinary `ok` -- so billing/measure can tell readiness
                // cost apart from real work. `building` is read straight
                // off the result shape `building_result` builds; `waited`
                // is read off the `waited_ms=` call-record line
                // `resolve_env_readiness`'s callers log only when they
                // actually waited (waited_ms > 0).
                let call_outcome = if value.get("status").and_then(Value::as_str)
                    == Some("building")
                {
                    "building"
                } else if log
                    .0
                    .lock()
                    .map(|g| g.iter().any(|l| l.starts_with("waited_ms=")))
                    .unwrap_or(false)
                {
                    "waited"
                } else {
                    "ok"
                };

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
                        call_outcome,
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
                    duration_ms, status = "ok", outcome = call_outcome, mcp_name_mismatch,
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
                        "error",
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
                        "timeout",
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

        // PRD-mcphost-sandbox-ready requirement 3 (AC3): same first-try
        // rejection as `host.tool_publish`, before this tool's kind is ever
        // dispatched to.
        if let Some(status) = kind.sandbox_status()
            && !status.ready
        {
            return Err(AppError::sandbox_unavailable(&status));
        }

        let descriptor = kind.describe(&row.spec);
        if let Ok(validator) = jsonschema::validator_for(&descriptor.input_schema)
            && let Err(e) = validator.validate(&call_args)
        {
            // Requirement 1/AC3 (PRD-mcphost-python-kind-runtime): phase
            // `args_coercion`, naming the argument and both types -- shared
            // with every kind via `describe_args_error`, wired through
            // `AppError::Structured` (not `ArgsInvalid`) so the extra `data`
            // fields survive into the JSON-RPC response; the wire code stays
            // pinned to exactly `args_invalid` either way.
            let data = describe_args_error(&e);
            return Err(AppError::Structured {
                code: "args_invalid",
                message: e.to_string(),
                data,
            });
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
            tool_name: Some(local_name.clone()),
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
            Ok(Ok(value)) => {
                let mut value = crate::secrets::redact_keys(&value, &["tenant_key"]);
                // PRD-mcphost-result-envelope-contract requirement 4, AC4:
                // before publish, name any declared output field this
                // spec's implementation buries -- `kind.declared_outputs`
                // is `Vec::new()` (no report at all) for a spec that
                // declares none, so this is invisible to every tool
                // published before this PRD.
                let declared = kind.declared_outputs(&row.spec);
                if !declared.is_empty()
                    && let Some(envelope) = crate::kinds::envelope_report(
                        &declared,
                        kind.payload_from_call_result(&value),
                        kind.source_for_output_search(&value),
                    )
                    && let Value::Object(map) = &mut value
                {
                    map.insert("envelope".to_string(), envelope);
                }
                Ok(value)
            }
            Ok(Err(kind_err)) => Err(AppError::from(kind_err)),
            Err(_elapsed) => Err(AppError::CallTimeout),
        }
    }

    /// `host.bridge_test` (PRD-mcphost-rest-bridge P1 requirement, AC6):
    /// `host.tool_test`'s sibling for a spec that hasn't been published
    /// yet -- "test before deploy" for a REST-bridge `upstream` spec (or
    /// any `http`-kind spec). Unlike `tool_test`, it takes the `spec`
    /// argument directly rather than looking a tool up by name, so nothing
    /// is stored in the tenant's tool table: an invalid spec fails at
    /// `validate` and a valid one performs the real dry-run call exactly
    /// like `tool_test` (real upstream, no `calls` row, no persisted
    /// logs, secrets redacted from the echoed request).
    async fn bridge_test(&self, tenant: &Tenant, args: Value) -> Result<Value, AppError> {
        let spec = args
            .get("spec")
            .cloned()
            .ok_or_else(|| AppError::InvalidArgs("missing required argument 'spec'".into()))?;
        let call_args = args
            .get("args")
            .cloned()
            .unwrap_or_else(|| Value::Object(Default::default()));

        let kind: Arc<dyn Kind> = self.state.kinds.get("http").ok_or_else(|| {
            AppError::Internal("the 'http' kind is not registered on this host".into())
        })?;

        // AC6's "invalid bridge spec ... reports the failure class": the
        // same `validate` a real `host.tool_publish` would run, so a spec
        // that would be rejected at publish time fails identically here,
        // before any request is attempted.
        if let Err(e) = kind.validate(&spec) {
            return Err(AppError::from(e));
        }

        let descriptor = kind.describe(&spec);
        if let Ok(validator) = jsonschema::validator_for(&descriptor.input_schema)
            && let Err(e) = validator.validate(&call_args)
        {
            let data = describe_args_error(&e);
            return Err(AppError::Structured {
                code: "args_invalid",
                message: e.to_string(),
                data,
            });
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
            tool_name: None,
        };

        match tokio::time::timeout(self.state.call_timeout, kind.call(&spec, call_args, &ctx)).await
        {
            Ok(Ok(value)) => Ok(crate::secrets::redact_keys(&value, &["tenant_key"])),
            Ok(Err(kind_err)) => Err(AppError::from(kind_err)),
            Err(_elapsed) => Err(AppError::CallTimeout),
        }
    }

    /// `host.spec_test` (PRD-mcphost-tool-test): dry-runs a `kind` + `spec`
    /// pair that has never been published -- up to
    /// [`crate::kinds::MAX_TEST_INVOCATIONS`] example invocations through
    /// [`crate::kinds::run_spec_test`], the exact same [`Kind::call`] entry
    /// point a published call dispatches to (AC8), each with
    /// `ctx.test_mode = true` so a kind that renders a request (`http`)
    /// echoes it back. No `tools` row is ever written for `spec`.
    ///
    /// Distinct from [`Self::tool_test`] (which dry-runs an
    /// *already-published* tool by name) and named `spec_test` rather than
    /// reusing that name: PRD-mcphost-tool-test's acceptance criteria
    /// describe a pre-publish, multi-invocation, kind-agnostic dry run
    /// (`kind`/`spec`/`invocations` in, per-invocation results out) under
    /// the RPC name `host.tool_test` -- but `host.tool_test` already shipped
    /// on this host with a different, incompatible contract (post-publish,
    /// single invocation, looked up by `name`) that other ACs
    /// (`quickstart`'s own worked example, `host.tool_run`'s sibling
    /// docs) already depend on. Reusing the name would either break that
    /// shipped contract or silently overload one RPC with two argument
    /// shapes; `host.spec_test` delivers this PRD's actual capability
    /// (`host.bridge_test`'s "test a spec you haven't published" idea,
    /// generalized from `http`-only to every kind, and from one invocation
    /// to up to five) without either.
    async fn spec_test(&self, tenant: &Tenant, args: Value) -> Result<Value, AppError> {
        let kind_name = args
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::InvalidArgs("missing required argument 'kind'".into()))?
            .to_string();
        let spec = args
            .get("spec")
            .cloned()
            .ok_or_else(|| AppError::InvalidArgs("missing required argument 'spec'".into()))?;
        let invocations = args
            .get("invocations")
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| {
                AppError::InvalidArgs("missing required argument 'invocations'".into())
            })?;

        // AC7: refused naming the limit, zero invocations run -- checked
        // before any other work (spec validation included).
        if invocations.len() > MAX_TEST_INVOCATIONS {
            return Err(AppError::Structured {
                code: "too_many_invocations",
                message: format!(
                    "at most {MAX_TEST_INVOCATIONS} invocations per host.spec_test call; got {}",
                    invocations.len()
                ),
                data: json!({"limit": MAX_TEST_INVOCATIONS}),
            });
        }

        // AC5: same error taxonomy as host.tool_publish -- spec-size, then
        // kind lookup, then sandbox readiness, then spec validation, in the
        // same order control::tool_publish checks them.
        let spec_bytes = serde_json::to_vec(&spec)
            .map_err(|e| AppError::Internal(format!("spec serialize: {e}")))?
            .len();
        if spec_bytes > MAX_SPEC_BYTES {
            return Err(AppError::SpecTooLarge(spec_bytes));
        }

        let kind: Arc<dyn Kind> =
            self.state
                .kinds
                .get(&kind_name)
                .ok_or_else(|| AppError::UnknownKind {
                    requested: kind_name.clone(),
                    registered: self.state.kinds.names(),
                })?;

        if let Some(status) = kind.sandbox_status()
            && !status.ready
        {
            return Err(AppError::sandbox_unavailable(&status));
        }

        if let Some(err) = AppError::from_kind_violations(kind.validate_all(&spec)) {
            return Err(err);
        }
        kind.validate_async(&spec).await?;

        // AC6: refused exactly as a normal call at quota, checked once
        // before any invocation executes (not per-invocation) -- the same
        // gate a real dispatched call passes through.
        self.check_calls_quota(tenant).await?;

        let descriptor = kind.describe(&spec);
        let requirements = kind.requirements(&spec);

        let secrets = build_secret_resolver(&self.state, tenant.id).await?;
        let call_timeout = self.state.call_timeout;
        let tenant_id = tenant.id;
        let namespace = tenant.namespace.clone();
        // AC12: unlike `host.tool_test`/`host.bridge_test` (deliberately
        // out-of-band debug calls, see `Self::tool_test`'s own doc comment),
        // a `host.spec_test` invocation's log output IS captured and
        // persisted -- one `BufferedLog` per invocation, built up front so
        // `ctx_factory` (called once per invocation, in order, by
        // `run_spec_test`) can hand each its own and this function can read
        // every one back once `run_spec_test` returns.
        let log_bufs: Vec<Arc<BufferedLog>> = invocations
            .iter()
            .map(|_| Arc::new(BufferedLog(std::sync::Mutex::new(Vec::new()))))
            .collect();
        let mut log_bufs_iter = log_bufs.iter().cloned();
        let results = run_spec_test(&kind, &spec, &invocations, call_timeout, || CallCtx {
            tenant_id,
            namespace: namespace.clone(),
            secrets: secrets.clone(),
            deadline: Instant::now() + call_timeout,
            log: log_bufs_iter
                .next()
                .map(|buf| buf as Arc<dyn CallLog>)
                .unwrap_or_else(|| Arc::new(NullLog)),
            test_mode: true,
            resources: Arc::new(NullResourceSink),
            tool_name: None,
        })
        .await;

        // AC6: each executed invocation is metered like a normal call (a
        // `calls` row, counting toward `calls_per_day`) even though no
        // `tools` row is ever written for this ad hoc spec. AC12: the same
        // synthetic per-kind name also buckets this call's persisted log
        // lines (below), so `host.tool_logs` can serve them back without a
        // `tools` row existing for it.
        let test_log_name = format!("__spec_test__.{kind_name}");
        for result in &results {
            let ok = result.get("ok").and_then(Value::as_bool).unwrap_or(false);
            let error_class = result
                .get("code")
                .and_then(Value::as_str)
                .map(str::to_string);
            let duration_ms = result
                .get("duration_ms")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            // `host.spec_test` runs a pre-publish spec, so a `building`
            // result here is possible (a fresh spec against a
            // never-before-seen requirements set) but not the common case
            // requirement 6 targets; `ok`/`error` covers it -- no
            // `waited_ms=` inspection since this loop has no per-invocation
            // log buffer handle at this point.
            let call_outcome = if ok { "ok" } else { "error" };
            let _ = self
                .state
                .db
                .record_call(
                    tenant.id,
                    test_log_name.clone(),
                    duration_ms,
                    ok,
                    error_class,
                    None,
                    None,
                    call_outcome,
                )
                .await;
        }

        // AC12: each invocation's captured log lines, marked `[test]` so
        // they read as distinguishable from a production call's own lines
        // in the same `host.tool_logs` bucket.
        for buf in &log_bufs {
            let lines = buf.0.lock().map(|g| g.clone()).unwrap_or_default();
            for line in lines {
                let _ = self
                    .state
                    .db
                    .append_log(tenant.id, test_log_name.clone(), format!("[test] {line}"))
                    .await;
            }
        }

        // PRD-mcphost-tool-kind-honor requirement 2 (AC3): unlike
        // `host.tool_publish`, a disagreement here never aborts the dry
        // run (a spec whose required fields actually contradict the
        // requested kind already fails `validate_all` above, same as
        // publish would) -- this only surfaces what the spec's shape
        // implies alongside what was requested, for a spec that validates
        // under the requested kind despite also carrying the other kind's
        // signal (e.g. an echo spec with a stray `source` field).
        let kind_report = match crate::kinds::infer::infer_kind_signal(&spec) {
            Some(signal) if signal.kind != kind_name => json!({
                "resolved": signal.kind,
                "requested": kind_name.clone(),
                "reason": signal.reason,
            }),
            _ => json!({"resolved": kind_name.clone(), "requested": kind_name.clone()}),
        };

        Ok(crate::secrets::redact_keys(
            &json!({
                "kind": kind_report,
                "args_schema": descriptor.input_schema,
                "requirements": requirements,
                "invocations": results,
            }),
            &["tenant_key"],
        ))
    }

    /// `host.tool_run` (PRD-mcphost-code-tools-warm-pool requirement 3,
    /// AC6/AC7): a debug run, dispatching to `Kind::tool_run` rather than
    /// `Kind::call` -- distinct raw-stdout/stderr/exit-code response shape,
    /// no `calls` row, no metering, and its own 30-per-minute-per-tenant
    /// rate limit checked before the sandbox ever runs (so the 31st call
    /// costs nothing, not even a build-state lookup).
    async fn tool_run(&self, tenant: &Tenant, args: Value) -> Result<Value, AppError> {
        if !self.state.tool_run_limiter.allow(tenant.id) {
            return Err(AppError::Structured {
                code: "rate_limited",
                message: "host.tool_run is limited to 30 calls per tenant per minute".to_string(),
                data: json!({"retry_after_s": 60}),
            });
        }

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

        // PRD-mcphost-sandbox-ready requirement 3 (AC3): same first-try
        // rejection as `host.tool_publish`, before this tool's kind is ever
        // dispatched to.
        if let Some(status) = kind.sandbox_status()
            && !status.ready
        {
            return Err(AppError::sandbox_unavailable(&status));
        }

        let descriptor = kind.describe(&row.spec);
        if let Ok(validator) = jsonschema::validator_for(&descriptor.input_schema)
            && let Err(e) = validator.validate(&call_args)
        {
            // Requirement 1/AC3 (PRD-mcphost-python-kind-runtime): phase
            // `args_coercion`, naming the argument and both types -- shared
            // with every kind via `describe_args_error`, wired through
            // `AppError::Structured` (not `ArgsInvalid`) so the extra `data`
            // fields survive into the JSON-RPC response; the wire code stays
            // pinned to exactly `args_invalid` either way.
            let data = describe_args_error(&e);
            return Err(AppError::Structured {
                code: "args_invalid",
                message: e.to_string(),
                data,
            });
        }

        let secrets = build_secret_resolver(&self.state, tenant.id).await?;
        let ctx = CallCtx {
            tenant_id: tenant.id,
            namespace: tenant.namespace.clone(),
            secrets,
            deadline: Instant::now() + self.state.call_timeout,
            log: Arc::new(NullLog) as Arc<dyn CallLog>,
            test_mode: false,
            resources: Arc::new(NullResourceSink),
            tool_name: Some(local_name.clone()),
        };

        let start = Instant::now();
        let outcome = tokio::time::timeout(
            self.state.call_timeout,
            kind.tool_run(&row.spec, call_args, &ctx),
        )
        .await;
        let duration_ms = start.elapsed().as_millis() as i64;

        match outcome {
            Ok(Ok(mut value)) => {
                if let Some(obj) = value.as_object_mut() {
                    obj.insert("duration_ms".to_string(), json!(duration_ms));
                }
                Ok(value)
            }
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
        let mut instructions = String::from(
            "Call `signup` with a display name to receive a bearer key. The `host.*` \
                 control plane -- including `host.tool_publish` and `host.tool_call` -- is \
                 already visible in this tools/list, before you have a key. Pass the key \
                 `signup` returns as the `tenant_key` argument on every call after that; no \
                 reconnect and no Authorization header is required. Result envelope contract: \
                 when a spec declares `outputs` (field names its tool emits, or a map from \
                 field name to the exact `$.a.b[0].c`-style path to read it from), each is \
                 readable at `result.payload.<field>` for every kind, regardless of how deep \
                 the tool's own response nests it -- run `host.tool_test` before publishing to \
                 see which declared fields your implementation buries. \
                 host.tool_publish's `kind` argument is honored exactly as given -- an inline \
                 `source` field only publishes as `python` and `upstream`/`method`+`url` fields \
                 only publish as `http` -- so request the kind your task needs and a mismatch \
                 returns a `kind_mismatch` error naming the disagreeing spec element instead of \
                 silently publishing the other kind.",
        );
        // PRD-mcphost-sandbox-ready P1 requirement 7 (AC8): named here too,
        // not just in host.tool_publish's own description -- a client that
        // reads `host.get_info`'s instructions before publishing anything
        // must see this before it ever tries.
        if let Some(status) = self
            .state
            .kinds
            .all()
            .find_map(|k| k.sandbox_status())
            .filter(|s| !s.ready)
        {
            instructions.push_str(&format!(
                " NOTE: this host's sandboxed kinds are currently rejected with \
                 sandbox_unavailable ({}) -- publish echo or http instead.",
                status.detail
            ));
        }
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(instructions)
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
                tools.extend(host_tools(&self.state.kinds, false));
                (tools, TOOLS_LIST_TTL_MS_STEADY)
            }
            Auth::Admin => (admin_tools(), TOOLS_LIST_TTL_MS_STEADY),
            Auth::Tenant(tenant) => {
                let mut tools = host_tools(&self.state.kinds, true);
                let rows = self
                    .state
                    .db
                    .list_tools(tenant.id)
                    .await
                    .map_err(AppError::into_error_data)?;
                for row in rows {
                    if let Some(kind) = self.state.kinds.get(&row.kind) {
                        let descriptor = kind.describe(&row.spec);
                        // PRD-mcphost-tool-kind-honor requirement 5 (AC6):
                        // additive `_meta.kind` on the wire `Tool` -- the
                        // same value `host.tool_list` already reports for
                        // this row -- so a caller reading the standard
                        // `tools/list` response, not just `host.tool_list`,
                        // can verify what kind actually shipped.
                        let mut meta = rmcp::model::MetaObject::new();
                        meta.0.insert("kind".to_string(), json!(row.kind));
                        tools.push(
                            Tool::new(
                                format!("{}.{}", tenant.namespace, row.name),
                                descriptor.description,
                                value_to_json_object(descriptor.input_schema),
                            )
                            .with_meta(meta),
                        );
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
        // PRD-mcphost-tenant-attribution requirement 2's "first
        // authenticated session if signup preceded capture" fallback: an
        // already-authenticated tenant with no captured client yet gets
        // one more chance, on whatever call this happens to be, from this
        // same session's peer info. `Db::set_tenant_client_info` itself
        // guards against a race with signup's own capture (its `WHERE
        // client_name IS NULL` only ever writes once). Best-effort: a
        // failure here must never fail the call it rides along with.
        if let Auth::Tenant(tenant) = &auth
            && tenant.client_name.is_none()
            && let Some((name, version)) = peer_client_info(&ctx)
        {
            let _ = self.state.db.set_tenant_client_info(tenant.id, name, version).await;
        }
        // Requirements 16/17: redacted by key name, recursively, exactly
        // once here, so every dispatch branch below -- a `host.*` tool, an
        // `admin.*` tool, or a direct namespaced call -- works from
        // arguments that can no longer carry the key, at any depth (AC19),
        // rather than each callee having to remember to do it itself.
        let args = crate::secrets::redact_keys(&raw_args, &["tenant_key"]);

        let outcome: Result<Value, AppError> = match (&auth, body_name.as_str()) {
            (_, "signup") => {
                let (client_name, client_version) = match peer_client_info(&ctx) {
                    Some((name, version)) => (Some(name), Some(version)),
                    None => (None, None),
                };
                control::signup(
                    &self.state,
                    &args,
                    &source,
                    control::SignupAttribution {
                        synthetic_header: synthetic_header(parts).as_deref(),
                        client_name: client_name.as_deref(),
                        client_version: client_version.as_deref(),
                        user_agent: user_agent_header(parts).as_deref(),
                    },
                )
                .await
            }
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
            // PRD-grand-loop-billing AC1: "billing.plans (anonymous and
            // tenant)" -- reachable exactly like host.quickstart, before
            // signup and regardless of auth, since it carries no
            // tenant-specific data.
            (_, "billing.plans") => Ok(crate::billing::plans(&self.state)),
            (Auth::Anonymous | Auth::Invalid, _) => Err(AppError::Unauthorized),
            (Auth::Admin, name) if name.starts_with("admin.") => {
                self.dispatch_admin_tool(name, args).await
            }
            (Auth::Admin, _) => Err(AppError::Forbidden),
            (Auth::Tenant(_), name) if name.starts_with("admin.") => {
                let _ = name;
                Err(AppError::Forbidden)
            }
            (Auth::Tenant(tenant), name)
                if name.starts_with("host.") || name.starts_with("billing.") =>
            {
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
