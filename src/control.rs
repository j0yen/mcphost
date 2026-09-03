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
