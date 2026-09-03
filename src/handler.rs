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
use crate::kinds::{CallCtx, CallLog, Kind, SecretResolver};
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

fn host_tools() -> Vec<Tool> {
    vec![
        Tool::new(
            "host.whoami",
            "Return the calling tenant's identity.",
            schema(json!({}), &[]),
        ),
        Tool::new(
            "host.tool_publish",
            "Publish a tool of a registered kind under this tenant's namespace.",
            schema(
                json!({
                    "name": {"type": "string"},
                    "kind": {"type": "string"},
                    "spec": {"type": "object"},
                }),
                &["name", "kind", "spec"],
            ),
        ),
        Tool::new(
            "host.tool_list",
            "List this tenant's published tools.",
            schema(json!({}), &[]),
        ),
        Tool::new(
            "host.tool_remove",
            "Remove a published tool by its local name.",
            schema(json!({"name": {"type": "string"}}), &["name"]),
        ),
        Tool::new(
            "host.tool_logs",
            "Return the most recent log lines for one of this tenant's tools.",
            schema(
                json!({"name": {"type": "string"}, "limit": {"type": "integer"}}),
                &["name"],
            ),
        ),
        Tool::new(
            "host.usage",
            "Calls, errors and duration percentiles for this tenant over a window.",
            schema(json!({"window": {"type": "string"}}), &[]),
        ),
        Tool::new(
            "host.secret_set",
            "Store an encrypted secret value under this tenant's namespace.",
            schema(
                json!({"name": {"type": "string"}, "value": {"type": "string"}}),
                &["name", "value"],
            ),
        ),
        Tool::new(
            "host.secret_list",
            "List this tenant's secret names (never their values).",
            schema(json!({}), &[]),
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
            "host.usage" => control::usage(&self.state, tenant, &args).await,
            "host.secret_set" => control::secret_set(&self.state, tenant, &args).await,
            "host.secret_list" => control::secret_list(&self.state, tenant).await,
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
            return Err(AppError::InvalidArgs(e.to_string()));
        }

        let secrets = build_secret_resolver(&self.state, tenant.id).await?;
        let log = Arc::new(BufferedLog(std::sync::Mutex::new(Vec::new())));
        let ctx = CallCtx {
            tenant_id: tenant.id,
            namespace: tenant.namespace.clone(),
            secrets,
            deadline: Instant::now() + self.state.call_timeout,
            log: log.clone() as Arc<dyn CallLog>,
        };

        let start = Instant::now();
        if mcp_name_mismatch {
            tracing::warn!(tenant = %tenant.namespace, tool = %local_name, "Mcp-Name header does not match call body's tool name");
        }
        let outcome =
            tokio::time::timeout(self.state.call_timeout, kind.call(&row.spec, args, &ctx)).await;
        let duration_ms = start.elapsed().as_millis() as i64;

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
                    .record_call(tenant.id, local_name.to_string(), duration_ms, true, None)
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
}

impl ServerHandler for McpHostHandler {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(
                "Call `signup` with a display name to receive a bearer key and namespace, \
                 then reconnect with `Authorization: Bearer <key>` to see the `host.*` control plane.",
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

        let result = match auth {
            Auth::Anonymous | Auth::Invalid => ListToolsResult::with_all_items(vec![signup_tool()]),
            Auth::Admin => ListToolsResult::with_all_items(admin_tools()),
            Auth::Tenant(tenant) => {
                let mut tools = host_tools();
                let rows = self
                    .state
                    .db
                    .list_tools(tenant.id)
                    .await
                    .map_err(AppError::into_error_data)?;
                let mut most_recent_publish_secs: i64 = 0;
                for row in &rows {
                    let created = row
                        .created_at
                        .strip_prefix("unix:")
                        .and_then(|s| s.split('.').next())
                        .and_then(|s| s.parse::<i64>().ok())
                        .unwrap_or(0);
                    most_recent_publish_secs = most_recent_publish_secs.max(created);
                }
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
                let recent_change =
                    now_unix() - most_recent_publish_secs < TOOLS_LIST_TTL_GRACE_SECS as i64;
                let ttl_ms = if most_recent_publish_secs > 0 && recent_change {
                    0
                } else {
                    TOOLS_LIST_TTL_MS_STEADY
                };
                ListToolsResult::with_all_items(tools)
                    .with_ttl_ms(ttl_ms)
                    .with_cache_scope(CacheScope::Private)
            }
        };
        Ok(result)
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

        let auth = resolve_auth(&self.state, parts)
            .await
            .map_err(AppError::into_error_data)?;

        let args = Value::Object(request.arguments.unwrap_or_default());

        let outcome: Result<Value, AppError> = match (&auth, body_name.as_str()) {
            (_, "signup") => control::signup(&self.state, &args, &source).await,
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
