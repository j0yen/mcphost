//! Business logic for `signup` and the `host.*` control plane. Pure
//! `AppState` + arguments in, `serde_json::Value` (or [`AppError`]) out —
//! `handler.rs` is the only place that touches `rmcp` wire types.

use serde_json::{Value, json};

use crate::auth::{generate_key, generate_namespace, hash_key};
use crate::db::Tenant;
use crate::errors::AppError;
use crate::state::{AppState, MAX_SPEC_BYTES, MAX_TOOLS_PER_TENANT, validate_tool_name};

fn arg_str(args: &Value, name: &str) -> Result<String, AppError> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppError::InvalidArgs(format!("missing required argument '{name}'")))
}

fn arg_str_opt(args: &Value, name: &str) -> Option<String> {
    args.get(name).and_then(Value::as_str).map(str::to_string)
}

pub async fn signup(state: &AppState, args: &Value, source_ip: &str) -> Result<Value, AppError> {
    let display_name = arg_str(args, "name")?;

    let since = crate::state::now_unix() - crate::state::SIGNUP_RATE_LIMIT_WINDOW_SECS;
    let recent = state
        .db
        .signup_count_since(source_ip.to_string(), since)
        .await?;
    if recent >= crate::state::SIGNUP_RATE_LIMIT_PER_HOUR {
        return Err(AppError::RateLimited);
    }

    let key = generate_key();
    let namespace = generate_namespace();
    let key_hash = hash_key(&key);
    state.db.record_signup_event(source_ip.to_string()).await?;
    let tenant = state
        .db
        .create_tenant(display_name, namespace.clone(), key_hash)
        .await?;

    Ok(json!({
        "tenant": tenant.namespace,
        "key": key,
        "namespace": namespace,
        "endpoint": format!("{}/mcp", state.public_url.trim_end_matches('/')),
        // PRD-mcphost-session-key requirement 3 / AC4: told by the payload
        // it is already reading, not a reconnect instruction it cannot
        // follow (this key never attaches to a connection property; the
        // Claude Agent SDK's mcp_servers config is fixed for the session).
        "usage": "Pass this key as the `tenant_key` argument on every tools/call from here \
            on -- e.g. host.tool_publish, host.tool_call -- no reconnect or \
            Authorization header needed.",
    }))
}

pub fn whoami(tenant: &Tenant) -> Value {
    json!({
        "tenant": tenant.namespace,
        "namespace": tenant.namespace,
        "display_name": tenant.display_name,
        "created_at": tenant.created_at,
        "disabled": tenant.disabled,
    })
}

pub async fn tool_publish(
    state: &AppState,
    tenant: &Tenant,
    args: &Value,
) -> Result<Value, AppError> {
    let name = arg_str(args, "name")?;
    let kind_name = arg_str(args, "kind")?;
    let spec = args.get("spec").cloned().unwrap_or(Value::Null);

    validate_tool_name(&name)?;

    let spec_bytes = serde_json::to_vec(&spec)
        .map_err(|e| AppError::Internal(format!("spec serialize: {e}")))?
        .len();
    if spec_bytes > MAX_SPEC_BYTES {
        return Err(AppError::SpecTooLarge(spec_bytes));
    }

    let kind = state
        .kinds
        .get(&kind_name)
        .ok_or_else(|| AppError::UnknownKind {
            requested: kind_name.clone(),
            registered: state.kinds.names(),
        })?;
    kind.validate(&spec)?;
    kind.validate_async(&spec).await?;

    // Requirement 3 / AC3: every `secret.<name>` the spec references must
    // already exist for this tenant, checked before the tool is ever
    // stored. Kinds with no secret-templating concept (`echo`) return no
    // references here, so this is a no-op for them.
    let referenced = kind.referenced_secrets(&spec);
    if !referenced.is_empty() {
        let known = state.db.list_secret_names(tenant.id).await?;
        for secret_name in referenced {
            if !known.contains(&secret_name) {
                return Err(AppError::SecretMissing(secret_name));
            }
        }
    }

    // A re-publish of an existing name must not count against the limit.
    let already_exists = state.db.get_tool(tenant.id, name.clone()).await?.is_some();
    if !already_exists {
        let count = state.db.count_tools(tenant.id).await?;
        if count >= MAX_TOOLS_PER_TENANT {
            return Err(AppError::TooManyTools(count as usize));
        }
    }

    state
        .db
        .upsert_tool(tenant.id, name.clone(), kind_name.clone(), spec)
        .await?;

    Ok(json!({
        "name": format!("{}.{}", tenant.namespace, name),
        "kind": kind_name,
    }))
}

pub async fn tool_list(state: &AppState, tenant: &Tenant) -> Result<Value, AppError> {
    let rows = state.db.list_tools(tenant.id).await?;
    let tools: Vec<Value> = rows
        .into_iter()
        .map(|row| {
            json!({
                "name": format!("{}.{}", tenant.namespace, row.name),
                "kind": row.kind,
                "created_at": row.created_at,
            })
        })
        .collect();
    Ok(json!({ "tools": tools }))
}

pub async fn tool_remove(
    state: &AppState,
    tenant: &Tenant,
    args: &Value,
) -> Result<Value, AppError> {
    let name = arg_str(args, "name")?;
    let removed = state.db.remove_tool(tenant.id, name.clone()).await?;
    if !removed {
        return Err(AppError::ToolNotFound(name));
    }
    Ok(json!({ "removed": name }))
}

pub async fn tool_logs(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let name = arg_str(args, "name")?;
    if state.db.get_tool(tenant.id, name.clone()).await?.is_none() {
        return Err(AppError::ToolNotFound(name));
    }
    let limit = args
        .get("limit")
        .and_then(Value::as_i64)
        .unwrap_or(20)
        .clamp(1, 1000);
    let lines = state.db.tail_logs(tenant.id, name.clone(), limit).await?;
    Ok(json!({ "name": name, "lines": lines }))
}

pub async fn usage(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let window = arg_str_opt(args, "window").unwrap_or_else(|| "24h".to_string());
    let secs = crate::state::parse_window_secs(&window);
    let stats = state.db.usage(tenant.id, secs).await?;
    Ok(json!({
        "window": window,
        "calls": stats.calls,
        "errors": stats.errors,
        "p50_ms": stats.p50_ms,
        "p95_ms": stats.p95_ms,
    }))
}

pub async fn secret_set(
    state: &AppState,
    tenant: &Tenant,
    args: &Value,
) -> Result<Value, AppError> {
    let name = arg_str(args, "name")?;
    let value = arg_str(args, "value")?;
    let (ct, nonce) = state.secrets.encrypt(&value)?;
    state
        .db
        .upsert_secret(tenant.id, name.clone(), ct, nonce)
        .await?;
    Ok(json!({ "name": name, "set": true }))
}

pub async fn secret_list(state: &AppState, tenant: &Tenant) -> Result<Value, AppError> {
    let names = state.db.list_secret_names(tenant.id).await?;
    Ok(json!({ "names": names }))
}

/// AC19 / requirement 15: publish this tenant's `server.json` to the
/// configured registry API and serve it locally at
/// `/.well-known/mcp/<namespace>/server.json`. Refuses with a distinct
/// error when the feature flag is off (`AppError::RegistryDisabled`) or
/// this tenant's domain namespace has not been admin-verified
/// (`AppError::NamespaceUnverified`); a non-2xx from the registry API
/// propagates as `AppError::RegistryRejected`, which does NOT leave a
/// stale document being served (the DB write only happens after the
/// registry itself accepts it).
pub async fn registry_publish(
    state: &AppState,
    tenant: &Tenant,
    _args: &Value,
) -> Result<Value, AppError> {
    let registry = state.registry.as_ref().ok_or(AppError::RegistryDisabled)?;
    if !tenant.namespace_verified {
        return Err(AppError::NamespaceUnverified);
    }
    let domain_namespace = tenant
        .registry_namespace
        .clone()
        .ok_or(AppError::NamespaceUnverified)?;

    let endpoint_url = format!("{}/mcp", state.public_url.trim_end_matches('/'));
    let document =
        crate::registry::build_server_json(&domain_namespace, &tenant.display_name, &endpoint_url);

    let publish_url = format!("{}/v0/publish", registry.base_url.trim_end_matches('/'));
    let resp = state
        .http_client
        .post(&publish_url)
        .json(&document)
        .send()
        .await
        .map_err(|e| AppError::RegistryRejected(format!("request to registry API failed: {e}")))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(AppError::RegistryRejected(format!(
            "registry API returned HTTP {status}"
        )));
    }

    state
        .db
        .upsert_registry_document(tenant.id, tenant.namespace.clone(), document.clone())
        .await?;

    Ok(json!({
        "namespace": tenant.namespace,
        "domain_namespace": domain_namespace,
        "well_known_url": format!(
            "{}/.well-known/mcp/{}/server.json",
            state.public_url.trim_end_matches('/'),
            tenant.namespace,
        ),
        "server_json": document,
        "registry_status": status.as_u16(),
    }))
}
