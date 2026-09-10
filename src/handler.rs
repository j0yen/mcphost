//! The `rmcp::ServerHandler` implementation: `initialize`, `list_tools` and
//! `call_tool`. This is the only module that translates between `rmcp` wire
//! types and the plain `serde_json::Value` business logic in `control.rs`
//! and `admin.rs`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

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
    CallCtx, CallLog, Kind, KindError, KindRegistry, MAX_TEST_INVOCATIONS, NoState, NullLog,
    NullResourceSink, ResourceSink, SecretResolver, StateBackend, describe_args_error,
    run_spec_test,
};
use crate::state::{
    AppState, MAX_SPEC_BYTES, TOOLS_LIST_TTL_GRACE_SECS, TOOLS_LIST_TTL_MS_STEADY, now_unix,
};
use crate::{admin, control, tenant_state};

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
        // PRD-mcphost-auth-error-names-argument requirement 2 / AC3: unlike
        // the header path's `AppError::TenantDisabled` (unchanged --
        // ac08_admin_disable_and_forbidden.rs pins that), a disabled
        // tenant's key sent as the `tenant_key` argument reads as
        // `tenant_key_invalid`, the same as any other unrecognized key --
        // it never distinguishes "exists but disabled" from "doesn't
        // exist" for this path, so nothing about a guessed key's validity
        // leaks through it.
        Some(t) if t.disabled => Err(AppError::TenantKeyInvalid),
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

/// PRD-mcphost-publish-first-try requirement 1 (AC1) named `host.tool_test`
/// as the dry run to try first; PRD-mcphost-surface-fluidity requirement 6
/// (P1, AC7) replaced the original approach -- one paragraph *per registered
/// kind*, each with a full example spec inlined here -- with a pointer to
/// `host.quickstart(kind)`, which already renders that same
/// [`crate::kinds::Kind::example`] filled in with the caller's own namespace
/// (see `control::quickstart`'s `steps`). Kept under 600 characters total
/// (asserted by `tests/surface_ac07_tool_publish_description_length.rs`) so
/// it reads in one glance in a `tools/list` response; per-kind detail lives
/// in `host.quickstart` instead of being re-explained on every kind's own
/// publish.
fn tool_publish_description(kinds: &KindRegistry) -> String {
    let kind_names = kinds.names().join(", ");
    let mut out = format!(
        "Publish a tool of a registered kind ({kind_names}) under this tenant's namespace. \
         Call host.quickstart(kind) first for a filled-in example spec and the full \
         publish-to-call sequence."
    );
    // PRD-mcphost-sandbox-ready P1 requirement 7 (AC8): a client that reads
    // this description before publishing must still see, right here, that a
    // kind is currently rejected -- so it never sends the doomed call.
    // `sandbox_status()` is `None` for every kind with no sandbox concept
    // (`echo`, `http`), so this is a no-op for them.
    for name in kinds.names() {
        let Some(kind) = kinds.get(name) else {
            continue;
        };
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
        " Name must match ^[a-z][a-z0-9_]{1,40}$; a rejection names the failing field and a \
         corrected example. Try host.tool_test before a real call.",
    );
    out
}

/// PRD-mcphost-tool-test AC9: `host.spec_test`, unlike every other `host.*`
/// descriptor, must be absent from `tools/list` for an anonymous/invalid
/// caller (present, and callable, only once authenticated) -- `authenticated`
/// gates pushing it onto the returned vec; every other `host.*` tool is
/// unaffected and stays visible pre-auth (PRD-mcphost-session-key).
/// PRD-mcphost-surface-fluidity requirement 1 / AC2: the one-sentence case
/// each dry-run tool's description states before pointing at
/// `host.quickstart`'s `try_before_call` table (built by
/// `control::quickstart`) for the other three. Kept as constants -- rather
/// than inlined into the `Tool::new` calls below -- so the length test
/// (`tests/surface_ac02_dry_run_descriptions.rs`) and this function read the
/// exact same source, per the PRD's own Technical considerations.
const TOOL_TEST_DESC: &str =
    "Dry-run an already-published tool by name, no calls row written; for the other cases see host.quickstart.";
const BRIDGE_TEST_DESC: &str =
    "Dry-run an unpublished http spec against its real upstream; for the other cases see host.quickstart.";
const SPEC_TEST_DESC: &str =
    "Dry-run an unpublished spec of any kind with example invocations; for the other cases see host.quickstart.";
const TOOL_RUN_DESC: &str =
    "Debug-run a published python tool for stdout, stderr and exit code; for the other cases see host.quickstart.";

/// PRD-mcphost-surface-fluidity requirement 4 (AC5): the name set
/// `www/llms.txt`'s generated tool section is built from -- the
/// authenticated superset of `host_tools` (a strict superset of the
/// anonymous one; the only gated entry is `host.spec_test`). `signup` is
/// deliberately not included here: it's visible only pre-auth, and
/// `llms_txt::tenant_tool_names` adds it back so the *union* across both
/// auth states is what gets compared against a live `tools/list`. Pure and
/// synchronous -- no `AppState`, no DB -- so `mcphost llms-txt` and
/// integration tests can call it without a running server.
pub fn llms_txt_tool_names(kinds: &KindRegistry) -> Vec<String> {
    host_tools(kinds, true)
        .into_iter()
        .map(|t| t.name.to_string())
        .collect()
}

/// PRD-mcphost-tool-test AC9: `host.spec_test`, unlike every other `host.*`
/// descriptor, must be absent from `tools/list` for an anonymous/invalid
/// caller (present, and callable, only once authenticated) -- `authenticated`
/// gates pushing it onto the returned vec; every other `host.*` tool is
/// unaffected and stays visible pre-auth (PRD-mcphost-session-key).
///
/// PRD-mcphost-surface-fluidity requirement 3 / AC4: every property here
/// (required or not) carries a `description`, including `host.bridge_test.spec`
/// and `host.spec_test.invocations`, which previously had none -- see
/// `tests/surface_ac04_required_field_descriptions.rs`.
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
                    "name": {
                        "type": "string",
                        "description": "Local name for the new tool; must match ^[a-z][a-z0-9_]{1,40}$.",
                    },
                    "kind": {
                        "type": "string",
                        "description": "Which registered kind to publish under, e.g. echo, http, python.",
                    },
                    "spec": {
                        "type": "object",
                        "description": "The kind-specific spec object; see host.quickstart(kind) for a \
                            filled-in example.",
                    },
                }),
                &["name", "kind", "spec"],
            ),
        ),
        Tool::new(
            "host.quickstart",
            "Return the shortest ordered sequence of calls to a working tool of `kind`, \
             with your namespace and a filled-in example already substituted in, plus the \
             current limits and a try_before_call table naming the one dry-run tool for \
             each case. Read-only. Call this before host.tool_publish if you're not sure \
             what a spec should look like. Unauthenticated callers get the signup step \
             first.",
            host_schema(
                json!({
                    "kind": {
                        "type": "string",
                        "description": "Which registered kind to return a worked example for, \
                            e.g. echo, http, python.",
                    },
                }),
                &["kind"],
            ),
        ),
        Tool::new(
            "host.tool_list",
            "List this tenant's published tools.",
            host_schema(json!({}), &[]),
        ),
        Tool::new(
            "host.tool_remove",
            "Remove a published tool by its local name.",
            host_schema(
                json!({
                    "name": {"type": "string", "description": "Local name of the tool to remove."},
                }),
                &["name"],
            ),
        ),
        Tool::new(
            "host.tool_logs",
            "Return the most recent log lines for one of this tenant's tools.",
            host_schema(
                json!({
                    "name": {
                        "type": "string",
                        "description": "Local name of the tool whose log lines to return.",
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max lines to return, most recent first; default 20.",
                    },
                }),
                &["name"],
            ),
        ),
        Tool::new(
            "host.tool_test",
            TOOL_TEST_DESC,
            host_schema(
                json!({
                    "name": {
                        "type": "string",
                        "description": "Local name of the already-published tool to dry-run.",
                    },
                    "args": {
                        "type": "object",
                        "description": "Arguments to pass, same shape as a real call.",
                    },
                }),
                &["name", "args"],
            ),
        ),
        Tool::new(
            "host.bridge_test",
            BRIDGE_TEST_DESC,
            host_schema(
                json!({
                    "spec": {
                        "type": "object",
                        "description": "An http-kind spec, not yet published, e.g. \
                            {\"url\": \"https://api.example.com/items/{{id}}\", \"method\": \"GET\"}.",
                    },
                    "args": {
                        "type": "object",
                        "description": "Arguments to render into the spec, same shape as a real call.",
                    },
                }),
                &["spec", "args"],
            ),
        ),
        Tool::new(
            "host.tool_run",
            TOOL_RUN_DESC,
            host_schema(
                json!({
                    "name": {
                        "type": "string",
                        "description": "Local name of the already-published python tool to debug-run.",
                    },
                    "args": {
                        "type": "object",
                        "description": "Arguments to pass, same shape as a real call.",
                    },
                }),
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
                json!({
                    "name": {"type": "string", "description": "Local name of the tool to invoke."},
                    "args": {
                        "type": "object",
                        "description": "Arguments to pass, validated against the tool's own args_schema.",
                    },
                }),
                &["name", "args"],
            ),
        ),
        Tool::new(
            "host.usage",
            "Calls, errors and duration percentiles for this tenant over a window.",
            host_schema(
                json!({
                    "window": {
                        "type": "string",
                        "description": "Time window to summarize, e.g. \"24h\"; default 24h.",
                    },
                }),
                &[],
            ),
        ),
        Tool::new(
            "host.secret_set",
            "Store an encrypted secret value under this tenant's namespace.",
            host_schema(
                json!({
                    "name": {
                        "type": "string",
                        "description": "Secret name, referenced from a spec as secret.<name>.",
                    },
                    "value": {
                        "type": "string",
                        "description": "The secret value; stored AES-256-GCM encrypted, never returned.",
                    },
                }),
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
        // PRD-mcphost-tenant-state P0 requirement 2: a per-tenant store an
        // agent inspects and seeds from its own session, mirroring the
        // `mcphost.state` module a python tool gets from inside the
        // sandbox (not yet built -- see the crate's `tenant_state.rs`
        // module doc).
        Tool::new(
            "host.state.get",
            "Read one key from this tenant's key-value state namespace. Returns \
             found: false (not an error) if the key was never set.",
            host_schema(
                json!({
                    "key": {
                        "type": "string",
                        "description": "Key to read from this tenant's key-value state namespace.",
                    },
                }),
                &["key"],
            ),
        ),
        Tool::new(
            "host.state.set",
            "Write one key in this tenant's key-value state namespace; value may be any \
             JSON value. Overrun of the plan's state_bytes_max quota fails with \
             state_quota_exceeded and writes nothing.",
            host_schema(
                json!({
                    "key": {
                        "type": "string",
                        "description": "Key to write in this tenant's key-value state namespace.",
                    },
                    "value": {"description": "Any JSON value to store under key."},
                }),
                &["key", "value"],
            ),
        ),
        Tool::new(
            "host.state.delete",
            "Delete one key from this tenant's key-value state namespace.",
            host_schema(
                json!({
                    "key": {
                        "type": "string",
                        "description": "Key to delete from this tenant's key-value state namespace.",
                    },
                }),
                &["key"],
            ),
        ),
        Tool::new(
            "host.state.list",
            "List keys (with their current values) in this tenant's key-value state \
             namespace, optionally filtered by prefix.",
            host_schema(
                json!({
                    "prefix": {
                        "type": "string",
                        "description": "Only list keys starting with this prefix; default: all keys.",
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max keys to return; default 100.",
                    },
                }),
                &[],
            ),
        ),
        Tool::new(
            "host.state.table_create",
            "Declare (or replace the schema of) a table in this tenant's state store. \
             schema is {\"column\": \"text\"|\"integer\"|\"real\"|\"boolean\"|\"json\"}; \
             primary_key, if given, must name one of schema's columns -- an insert whose \
             row matches an existing row's primary_key value replaces it.",
            host_schema(
                json!({
                    "name": {
                        "type": "string",
                        "description": "Table name to declare, or replace the schema of.",
                    },
                    "schema": {
                        "type": "object",
                        "description": "Column name to type map, each type one of \
                            text|integer|real|boolean|json, e.g. {\"id\": \"integer\"}.",
                    },
                    "primary_key": {
                        "type": "string",
                        "description": "Column name (must be in schema) whose matching value \
                            replaces an existing row on insert; optional.",
                    },
                }),
                &["name", "schema"],
            ),
        ),
        Tool::new(
            "host.state.table_drop",
            "Drop a declared table and every row it holds.",
            host_schema(
                json!({
                    "name": {
                        "type": "string",
                        "description": "Name of the declared table to drop, with every row it holds.",
                    },
                }),
                &["name"],
            ),
        ),
        Tool::new(
            "host.state.insert",
            "Insert one row (an object) or several (an array of objects) into a declared \
             table. Each row is validated against the table's schema first -- a type \
             mismatch fails the whole call with state_schema_violation and writes nothing.",
            host_schema(
                json!({
                    "table": {"type": "string", "description": "Name of the declared table to insert into."},
                    "rows": {
                        "description": "One row (an object) or several (an array of objects), each \
                            validated against the table's schema.",
                    },
                }),
                &["table", "rows"],
            ),
        ),
        Tool::new(
            "host.state.query",
            "Read rows from a declared table, optionally filtered (where: \"field op value\", \
             ops = != < <= > >=, clauses joined by ' and '), ordered (order_by: \"field\" or \
             \"field desc\") and capped (limit).",
            host_schema(
                json!({
                    "table": {"type": "string", "description": "Name of the declared table to read from."},
                    "where": {
                        "type": "string",
                        "description": "Optional filter, e.g. \"age > 21\"; ops are != < <= > >=, \
                            clauses joined by ' and '.",
                    },
                    "order_by": {
                        "type": "string",
                        "description": "Optional \"field\" or \"field desc\" to sort by.",
                    },
                    "limit": {"type": "integer", "description": "Max rows to return; optional."},
                }),
                &["table"],
            ),
        ),
        Tool::new(
            "host.state.delete_rows",
            "Delete rows from a declared table matching an optional where filter (same \
             grammar as host.state.query); omitting where deletes every row in the table.",
            host_schema(
                json!({
                    "table": {
                        "type": "string",
                        "description": "Name of the declared table to delete rows from.",
                    },
                    "where": {
                        "type": "string",
                        "description": "Optional filter, same grammar as host.state.query; \
                            omit to delete every row.",
                    },
                }),
                &["table"],
            ),
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
            host_schema(
                json!({
                    "plan": {"type": "string", "description": "Which plan to check out; default: pro."},
                }),
                &[],
            ),
        ),
    ];
    if authenticated {
        tools.push(Tool::new(
            "host.spec_test",
            SPEC_TEST_DESC,
            host_schema(
                json!({
                    "kind": {
                        "type": "string",
                        "description": "Which registered kind the spec is for, e.g. echo, http, python.",
                    },
                    "spec": {
                        "type": "object",
                        "description": "The kind-specific spec object to test, not yet published; \
                            see host.quickstart(kind) for an example.",
                    },
                    "invocations": {
                        "type": "array",
                        "items": {"type": "object"},
                        "maxItems": MAX_TEST_INVOCATIONS,
                        "description": "Up to 5 example call-argument objects to run through the \
                            spec, e.g. [{\"msg\": \"hi\"}].",
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
        Tool::new(
            "admin.audit_log",
            "Paged, newest-first view of the admin_audit log (PRD-mcphost-provenance-audit \
             requirement 4) -- every admin-bearer mutation's actor, action, target, and \
             timestamp.",
            schema(
                json!({
                    "limit": {"type": "integer"},
                    "before_id": {"type": "integer"},
                }),
                &[],
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

/// `tenant_state.rs`'s own errors are always `AppError::Structured`,
/// `InvalidArgs`, or `InvalidSpec` (see its `arg_str`/`quota_exceeded`/
/// `schema_violation`/`table_not_found` helpers); those three round-trip
/// their fields unchanged into the matching `KindError` variant. Anything
/// else becomes an `Exec` carrying the message -- the same fallback shape
/// `From<KindError> for AppError` uses in the other direction.
fn app_error_to_kind_error(e: AppError) -> KindError {
    match e {
        AppError::Structured {
            code,
            message,
            data,
        } => KindError::Structured {
            code,
            message,
            data,
        },
        AppError::InvalidArgs(m) => KindError::InvalidArgs(m),
        AppError::InvalidSpec(m) => KindError::InvalidSpec(m),
        other => KindError::Exec(other.to_string()),
    }
}

/// PRD-mcphost-tenant-state requirement 3: bridges `Kind::call`'s
/// `CallCtx.state` to the real `tenant_state.rs` business logic for this
/// call's own tenant -- the same pattern `build_secret_resolver` uses for
/// `CallCtx.secrets`. `op` is one of the bare verb names `kinds::python`'s
/// `mcphost.state` sandbox module sends (`"get"`, `"set"`, `"delete"`,
/// `"list"`, `"table_create"`, `"table_drop"`, `"insert"`, `"query"`,
/// `"delete_rows"`) -- distinct from the dotted `host.state.*` tool names
/// `dispatch_control_tool` matches above, which is the *other* caller of
/// these same `tenant_state::state_*` functions.
struct TenantStateBridge {
    state: Arc<AppState>,
    tenant: Tenant,
}

#[async_trait::async_trait]
impl StateBackend for TenantStateBridge {
    async fn call(&self, op: &str, args: Value) -> Result<Value, KindError> {
        let result = match op {
            "get" => tenant_state::state_get(&self.state, &self.tenant, &args).await,
            "set" => tenant_state::state_set(&self.state, &self.tenant, &args).await,
            "delete" => tenant_state::state_delete(&self.state, &self.tenant, &args).await,
            "list" => tenant_state::state_list(&self.state, &self.tenant, &args).await,
            "table_create" => {
                tenant_state::state_table_create(&self.state, &self.tenant, &args).await
            }
            "table_drop" => tenant_state::state_table_drop(&self.state, &self.tenant, &args).await,
            "insert" => tenant_state::state_insert(&self.state, &self.tenant, &args).await,
            "query" => tenant_state::state_query(&self.state, &self.tenant, &args).await,
            "delete_rows" => {
                tenant_state::state_delete_rows(&self.state, &self.tenant, &args).await
            }
            other => Err(AppError::InvalidArgs(format!("unknown state op '{other}'"))),
        };
        result.map_err(app_error_to_kind_error)
    }
}

/// requirement 5 / AC7: the per-call tally `CountingStateBackend` builds up
/// as `mcphost.state`/`host.state.*` ops flow through this call's
/// `CallCtx.state`, read back after `Kind::call`/`Kind::tool_run` returns
/// to fill `host.tool_test`/`host.tool_run`'s `state: {reads, writes, keys,
/// tables}`. `get`/`list`/`query` count as reads; every op that can mutate
/// the store counts as a write, whether or not it ends up changing
/// anything (a `delete` of a key that was never set still counts -- the
/// caller asked to write, same as `record_call`'s `ok = 1` for a tool call
/// that ran but changed nothing).
#[derive(Default, Clone)]
struct StateTally {
    reads: i64,
    writes: i64,
    keys: Vec<String>,
    tables: Vec<String>,
}

impl StateTally {
    fn to_json(&self) -> Value {
        json!({
            "reads": self.reads,
            "writes": self.writes,
            "keys": self.keys,
            "tables": self.tables,
        })
    }
}

/// Wraps a real `StateBackend` (always a `TenantStateBridge` in practice)
/// to (a) tally reads/writes/keys/tables for `host.tool_test`/
/// `host.tool_run`'s `state` field and (b) optionally push one `CallLog`
/// line per write -- `host.tool_logs`' "state writes carry key or table and
/// byte delta" (requirement 5). `log` is `None` for `host.tool_test`
/// (which persists no logs at all, same as every other call it makes)
/// and `Some` for a real published call.
struct CountingStateBackend {
    inner: Arc<dyn StateBackend>,
    tally: std::sync::Mutex<StateTally>,
    log: Option<Arc<dyn CallLog>>,
}

impl CountingStateBackend {
    fn new(inner: Arc<dyn StateBackend>, log: Option<Arc<dyn CallLog>>) -> Self {
        Self {
            inner,
            tally: std::sync::Mutex::new(StateTally::default()),
            log,
        }
    }

    fn snapshot(&self) -> StateTally {
        self.tally.lock().map(|g| g.clone()).unwrap_or_default()
    }
}

/// `op` names one of `tenant_state.rs`'s own verbs, exactly as
/// `TenantStateBridge::call` (above) matches them.
const STATE_WRITE_OPS: &[&str] = &[
    "set",
    "delete",
    "table_create",
    "table_drop",
    "insert",
    "delete_rows",
];

#[async_trait::async_trait]
impl StateBackend for CountingStateBackend {
    async fn call(&self, op: &str, args: Value) -> Result<Value, KindError> {
        let key = args.get("key").and_then(Value::as_str).map(str::to_string);
        // `table_create`/`table_drop` name the table `name`; `insert`/
        // `query`/`delete_rows` name it `table` -- same split
        // `kinds::python`'s `mcphost.state` module and `host.state.*`'s own
        // schema use.
        let table = match op {
            "table_create" | "table_drop" => {
                args.get("name").and_then(Value::as_str).map(str::to_string)
            }
            _ => args.get("table").and_then(Value::as_str).map(str::to_string),
        };
        let is_write = STATE_WRITE_OPS.contains(&op);

        let result = self.inner.call(op, args).await?;

        if let Ok(mut guard) = self.tally.lock() {
            if is_write {
                guard.writes += 1;
            } else {
                guard.reads += 1;
            }
            if let Some(k) = &key
                && !guard.keys.contains(k)
            {
                guard.keys.push(k.clone());
            }
            if let Some(t) = &table
                && !guard.tables.contains(t)
            {
                guard.tables.push(t.clone());
            }
        }

        if is_write
            && let Some(log) = &self.log
        {
            let bytes_delta = result.get("bytes_delta").and_then(Value::as_i64).unwrap_or(0);
            let line = match (&key, &table) {
                (Some(k), _) => format!("state_write key={k} bytes_delta={bytes_delta}"),
                (None, Some(t)) => format!("state_write table={t} bytes_delta={bytes_delta}"),
                (None, None) => format!("state_write op={op} bytes_delta={bytes_delta}"),
            };
            log.log(&line);
        }

        Ok(result)
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
            "host.state.get" => tenant_state::state_get(&self.state, tenant, &args).await,
            "host.state.set" => tenant_state::state_set(&self.state, tenant, &args).await,
            "host.state.delete" => tenant_state::state_delete(&self.state, tenant, &args).await,
            "host.state.list" => tenant_state::state_list(&self.state, tenant, &args).await,
            "host.state.table_create" => {
                tenant_state::state_table_create(&self.state, tenant, &args).await
            }
            "host.state.table_drop" => {
                tenant_state::state_table_drop(&self.state, tenant, &args).await
            }
            "host.state.insert" => tenant_state::state_insert(&self.state, tenant, &args).await,
            "host.state.query" => tenant_state::state_query(&self.state, tenant, &args).await,
            "host.state.delete_rows" => {
                tenant_state::state_delete_rows(&self.state, tenant, &args).await
            }
            "billing.status" => crate::billing::status(&self.state, tenant).await,
            "billing.checkout" => crate::billing::checkout(&self.state, tenant, &args).await,
            other => Err(AppError::ToolNotFound(other.to_string())),
        }
    }

    async fn dispatch_admin_tool(&self, name: &str, args: Value) -> Result<Value, AppError> {
        let result = match name {
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
            "admin.audit_log" => admin::audit_log(&self.state, &args).await,
            other => Err(AppError::ToolNotFound(other.to_string())),
        };

        // PRD-mcphost-provenance-audit requirement 4: every admin-bearer
        // mutation appends to `admin_audit` from this one central point,
        // rather than each admin::* fn writing its own row -- the same
        // "one contract, many callers" reasoning `state::derive_origin`
        // follows for write-time origin.
        if let Ok(value) = &result
            && let Some((action, target, detail)) = admin::admin_audit_entry(name, &args, value)
        {
            let actor_key_id = self
                .state
                .admin_key
                .as_deref()
                .map(crate::auth::hash_key)
                .unwrap_or_default();
            if let Err(e) = self
                .state
                .db
                .record_admin_audit(actor_key_id, action, target, detail)
                .await
            {
                tracing::warn!(error = %e, tool = name, "failed to record admin_audit row");
            }
        }
        result
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

    /// PRD-mcphost-call-limits-honest requirement 1: the deadline this
    /// call's spec actually asks for (a kind-specific override, already
    /// bounded by that kind's own maximum -- see
    /// [`Kind::requested_timeout`]), or `AppState::call_timeout`'s default
    /// when the spec declares none. Every real dispatch site below resolves
    /// through this rather than reaching for `self.state.call_timeout`
    /// directly, so `CALL_TIMEOUT` is a default a spec can override, never a
    /// silent ceiling under it.
    fn resolve_call_timeout(&self, kind: &Arc<dyn Kind>, spec: &Value) -> Duration {
        kind.requested_timeout(spec)
            .unwrap_or(self.state.call_timeout)
    }

    /// PRD-mcphost-call-limits-honest requirement 3: `tenant`'s own
    /// concurrent-call admission cap, read from its plan -- `usize::MAX`
    /// (no per-tenant limiting) if the tenant's plan has somehow fallen out
    /// of the loaded catalog (should never happen; the same degrade
    /// `control::quickstart`'s own plan lookup already uses).
    fn concurrent_calls_cap(&self, tenant: &Tenant) -> usize {
        self.state
            .plans
            .get(&tenant.plan)
            .map(|plan| plan.concurrent_calls_per_tenant.max(0) as usize)
            .unwrap_or(usize::MAX)
    }

    /// Execute a published tenant tool (`<namespace>.<local-name>`),
    /// metering the call and enforcing its resolved deadline (requirement
    /// 1: the spec's own `timeout_s` when it declared one, else
    /// `AppState::call_timeout`'s default).
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
        let resolved_timeout = self.resolve_call_timeout(&kind, &row.spec);
        let ctx = CallCtx {
            tenant_id: tenant.id,
            namespace: tenant.namespace.clone(),
            secrets,
            deadline: Instant::now() + resolved_timeout,
            log: log.clone() as Arc<dyn CallLog>,
            test_mode: false,
            resources: resources.clone() as Arc<dyn ResourceSink>,
            tool_name: Some(local_name.to_string()),
            // requirement 5: a real call's state writes get one
            // `CallLog` line each (flushed into `host.tool_logs` below,
            // same as every other line this call's `Kind` logs); it
            // carries no `state` tally in its own return value -- that's
            // `host.tool_test`/`host.tool_run`-only (see those methods).
            state: Arc::new(CountingStateBackend::new(
                Arc::new(TenantStateBridge {
                    state: self.state.clone(),
                    tenant: tenant.clone(),
                }),
                Some(log.clone() as Arc<dyn CallLog>),
            )),
            // PRD-mcphost-composition requirement 1/2: every real
            // `tools/call`/`host.tool_call` dispatch is the root of its own
            // composition tree -- depth 0, a fresh per-tree children
            // counter, and the tenant table/kind registry a composing
            // `Kind` (`chain`) needs to dispatch its own children through
            // `kinds::compose_call`.
            compose_depth: 0,
            compose_children: Some(Arc::new(std::sync::atomic::AtomicU32::new(0))),
            compose_db: Some(self.state.db.clone()),
            compose_kinds: Some(self.state.kinds.clone()),
            concurrent_calls_per_tenant: self.concurrent_calls_cap(tenant),
        };

        let start = Instant::now();
        if mcp_name_mismatch {
            tracing::warn!(tenant = %tenant.namespace, tool = %local_name, "Mcp-Name header does not match call body's tool name");
        }
        let outcome = tokio::time::timeout(resolved_timeout, kind.call(&row.spec, args, &ctx)).await;
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
                        tenant.origin.clone(),
                        tenant.origin_detail.clone(),
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
                        tenant.origin.clone(),
                        tenant.origin_detail.clone(),
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
                        tenant.origin.clone(),
                        tenant.origin_detail.clone(),
                    )
                    .await;
                tracing::info!(
                    tenant = %tenant.namespace, method = "tools/call", tool = %local_name,
                    duration_ms, status = "error", error_class = "call_timeout", mcp_name_mismatch,
                );
                Err(AppError::CallTimeout(resolved_timeout.as_secs()))
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
        let state_backend = Arc::new(CountingStateBackend::new(
            Arc::new(TenantStateBridge {
                state: self.state.clone(),
                tenant: tenant.clone(),
            }),
            None,
        ));
        let resolved_timeout = self.resolve_call_timeout(&kind, &row.spec);
        let ctx = CallCtx {
            tenant_id: tenant.id,
            namespace: tenant.namespace.clone(),
            secrets,
            deadline: Instant::now() + resolved_timeout,
            log: Arc::new(NullLog) as Arc<dyn CallLog>,
            test_mode: true,
            resources: Arc::new(NullResourceSink),
            tool_name: Some(local_name.clone()),
            // requirement 5 / AC7: `host.tool_test` persists no logs (`log`
            // above is `NullLog`), but its *result* carries a `state` tally
            // -- read back from `state_backend` after the call below.
            state: state_backend.clone() as Arc<dyn StateBackend>,
            // PRD-mcphost-composition requirement 3/AC8: `chain`'s dry run
            // (`ctx.test_mode`) resolves only literal and `$.input.*`
            // mappings -- it never dispatches a step, so it never needs
            // `compose_db`/`compose_kinds` here.
            compose_depth: 0,
            compose_children: None,
            compose_db: None,
            compose_kinds: None,
            concurrent_calls_per_tenant: self.concurrent_calls_cap(tenant),
        };

        match tokio::time::timeout(resolved_timeout, kind.call(&row.spec, call_args, &ctx)).await {
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
                if let Value::Object(map) = &mut value {
                    map.insert("state".to_string(), state_backend.snapshot().to_json());
                }
                Ok(value)
            }
            Ok(Err(kind_err)) => Err(AppError::from(kind_err)),
            Err(_elapsed) => Err(AppError::CallTimeout(resolved_timeout.as_secs())),
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
        let resolved_timeout = self.resolve_call_timeout(&kind, &spec);
        let ctx = CallCtx {
            tenant_id: tenant.id,
            namespace: tenant.namespace.clone(),
            secrets,
            deadline: Instant::now() + resolved_timeout,
            log: Arc::new(NullLog) as Arc<dyn CallLog>,
            test_mode: true,
            resources: Arc::new(NullResourceSink),
            tool_name: None,
            // `host.bridge_test` always dispatches to the `http` kind
            // (fixed above), which has no notion of `mcphost.state` --
            // `NoState` is correct here, not a stand-in for a real backend.
            state: Arc::new(NoState),
            compose_depth: 0,
            compose_children: None,
            compose_db: None,
            compose_kinds: None,
            concurrent_calls_per_tenant: self.concurrent_calls_cap(tenant),
        };

        match tokio::time::timeout(resolved_timeout, kind.call(&spec, call_args, &ctx)).await {
            Ok(Ok(value)) => Ok(crate::secrets::redact_keys(&value, &["tenant_key"])),
            Ok(Err(kind_err)) => Err(AppError::from(kind_err)),
            Err(_elapsed) => Err(AppError::CallTimeout(resolved_timeout.as_secs())),
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
        let call_timeout = self.resolve_call_timeout(&kind, &spec);
        let concurrent_calls_per_tenant = self.concurrent_calls_cap(tenant);
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
        let state_backend: Arc<dyn StateBackend> = Arc::new(TenantStateBridge {
            state: self.state.clone(),
            tenant: tenant.clone(),
        });
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
            state: state_backend.clone(),
            compose_depth: 0,
            compose_children: None,
            compose_db: None,
            compose_kinds: None,
            concurrent_calls_per_tenant,
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
                    tenant.origin.clone(),
                    tenant.origin_detail.clone(),
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
        // requirement 5 / AC7: `host.tool_run`'s result carries a `state`
        // tally too (same shape as `host.tool_test`'s) -- read back from
        // `state_backend` after the call below.
        let state_backend = Arc::new(CountingStateBackend::new(
            Arc::new(TenantStateBridge {
                state: self.state.clone(),
                tenant: tenant.clone(),
            }),
            None,
        ));
        let resolved_timeout = self.resolve_call_timeout(&kind, &row.spec);
        let ctx = CallCtx {
            tenant_id: tenant.id,
            namespace: tenant.namespace.clone(),
            secrets,
            deadline: Instant::now() + resolved_timeout,
            log: Arc::new(NullLog) as Arc<dyn CallLog>,
            test_mode: false,
            resources: Arc::new(NullResourceSink),
            tool_name: Some(local_name.clone()),
            state: state_backend.clone() as Arc<dyn StateBackend>,
            // PRD-mcphost-composition: `host.tool_run` has no notion of a
            // composition tree of its own yet (see `CallCtx::compose_db`'s
            // doc) -- `chain`/`mcphost.call` are unavailable from here,
            // same as `host.tool_test`/`host.spec_test` above.
            compose_depth: 0,
            compose_children: None,
            compose_db: None,
            compose_kinds: None,
            concurrent_calls_per_tenant: self.concurrent_calls_cap(tenant),
        };

        let start = Instant::now();
        let outcome =
            tokio::time::timeout(resolved_timeout, kind.tool_run(&row.spec, call_args, &ctx)).await;
        let duration_ms = start.elapsed().as_millis() as i64;

        match outcome {
            Ok(Ok(mut value)) => {
                if let Some(obj) = value.as_object_mut() {
                    obj.insert("duration_ms".to_string(), json!(duration_ms));
                    obj.insert("state".to_string(), state_backend.snapshot().to_json());
                }
                Ok(value)
            }
            Ok(Err(kind_err)) => Err(AppError::from(kind_err)),
            Err(_elapsed) => Err(AppError::CallTimeout(resolved_timeout.as_secs())),
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
                 reconnect and no Authorization header is required. A `host.*` call with no \
                 `tenant_key` at all fails with `tenant_key_missing`, and one that doesn't \
                 match any tenant fails with `tenant_key_invalid`. Result envelope contract: \
                 when a spec declares `outputs` (field names its tool emits, or a map from \
                 field name to the exact `$.a.b[0].c`-style path to read it from), each is \
                 readable at `result.payload.<field>` for every kind, regardless of how deep \
                 the tool's own response nests it -- run `host.tool_test` before publishing to \
                 see which declared fields your implementation buries. \
                 host.tool_publish's `kind` argument is honored exactly as given -- an inline \
                 `source` field only publishes as `python` and `upstream`/`method`+`url` fields \
                 only publish as `http` -- so request the kind your task needs and a mismatch \
                 returns a `kind_mismatch` error naming the disagreeing spec element instead of \
                 silently publishing the other kind. Before you call a tool for the first \
                 time, see `host.quickstart`'s `try_before_call` table for which of the four \
                 dry-run tools (host.tool_test, host.bridge_test, host.spec_test, \
                 host.tool_run) fits your case.",
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
        //
        // PRD-mcphost-auth-error-names-argument requirement 1-3: this flag
        // is what lets the final `(Auth::Anonymous | Auth::Invalid, _)` arm
        // below tell a missing/invalid `tenant_key` argument apart from a
        // missing/invalid `Authorization` header -- `resolve_auth` only
        // ever returns `Anonymous` when no header was sent at all, so
        // reaching this branch already proves the header path isn't in
        // play for whatever `auth` becomes next.
        let via_tenant_key_arg = matches!(auth, Auth::Anonymous);
        if via_tenant_key_arg {
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
            // Anonymous/Invalid -> auth-error arm below, the same way
            // `signup` itself is.
            (Auth::Tenant(tenant), "host.quickstart") => {
                match control::quickstart(&self.state, Some(tenant), &args) {
                    Ok(mut result) => {
                        // PRD-mcphost-composition requirement 6: once this
                        // tenant has at least two tools of its own,
                        // `host.quickstart(kind="chain")`'s example wires
                        // the placeholder two-step spec onto them by name
                        // instead of `ChainKind::example`'s static
                        // `fetch_rows`/`write_rows` placeholders -- still
                        // just an example (`host.tool_publish` still has to
                        // be called to actually create it), but one that
                        // calls something the tenant already published.
                        // Best-effort: any lookup failure just leaves the
                        // generic placeholder spec.
                        if args.get("kind").and_then(Value::as_str) == Some("chain")
                            && let Ok(tools) = self.state.db.list_tools(tenant.id).await
                        {
                            let names: Vec<String> = tools
                                .into_iter()
                                .filter(|t| t.kind != "chain")
                                .map(|t| t.name)
                                .take(2)
                                .collect();
                            if let [first, second] = names.as_slice() {
                                result["steps"][0]["arguments"]["spec"] = json!({
                                    "steps": [
                                        {"tool": first, "args": {}},
                                        {"tool": second, "args": {}},
                                    ],
                                });
                            }
                        }
                        Ok(result)
                    }
                    Err(e) => Err(e),
                }
            }
            (_, "host.quickstart") => control::quickstart(&self.state, None, &args),
            // PRD-grand-loop-billing AC1: "billing.plans (anonymous and
            // tenant)" -- reachable exactly like host.quickstart, before
            // signup and regardless of auth, since it carries no
            // tenant-specific data.
            (_, "billing.plans") => Ok(crate::billing::plans(&self.state)),
            // PRD-mcphost-auth-error-names-argument requirement 1-3 /
            // AC1-4: three distinct auth failures now, not one. A header
            // was sent and didn't resolve (`!via_tenant_key_arg`) keeps the
            // original bearer-shaped text and its own `bearer_invalid`
            // code (AC4). No header at all falls through to the
            // `tenant_key` argument -- absent (or non-string) is
            // `tenant_key_missing` (AC1), present but unrecognized is
            // `tenant_key_invalid` (AC2/AC3, message never echoes the key).
            // Requirement 5 / AC6: the journal line carries the code and
            // the tool name -- never the key itself, which never appears
            // in `body_name` (the requested tool's name, not its
            // arguments).
            (Auth::Anonymous, _) if via_tenant_key_arg => {
                let err = AppError::TenantKeyMissing;
                tracing::warn!(code = err.code(), tool = %body_name, "host.* call refused: tenant_key missing");
                Err(err)
            }
            (Auth::Invalid, _) if via_tenant_key_arg => {
                let err = AppError::TenantKeyInvalid;
                tracing::warn!(code = err.code(), tool = %body_name, "host.* call refused: tenant_key invalid");
                Err(err)
            }
            (Auth::Anonymous | Auth::Invalid, _) => {
                let err = AppError::Unauthorized;
                tracing::warn!(code = err.code(), tool = %body_name, "call refused: missing or invalid Authorization header");
                Err(err)
            }
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
