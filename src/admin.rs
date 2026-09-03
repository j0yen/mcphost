//! Business logic for the `admin.*` tools, visible only to `$MCPHOST_ADMIN_KEY`.

use serde_json::{Value, json};

use crate::errors::AppError;
use crate::state::AppState;

fn arg_str(args: &Value, name: &str) -> Result<String, AppError> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppError::InvalidArgs(format!("missing required argument '{name}'")))
}

fn arg_str_opt(args: &Value, name: &str) -> Option<String> {
    args.get(name).and_then(Value::as_str).map(str::to_string)
}

pub async fn tenants(state: &AppState) -> Result<Value, AppError> {
    let rows = state.db.list_tenants().await?;
    let tenants: Vec<Value> = rows
        .into_iter()
        .map(|t| {
            json!({
                "tenant": t.namespace,
                "display_name": t.display_name,
                "created_at": t.created_at,
                "disabled": t.disabled,
            })
        })
        .collect();
    Ok(json!({ "tenants": tenants }))
}

pub async fn tenant_disable(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let tenant = arg_str(args, "tenant")?;
    let changed = state.db.set_tenant_disabled(tenant.clone(), true).await?;
    if !changed {
        return Err(AppError::ToolNotFound(format!("tenant {tenant}")));
    }
    Ok(json!({ "tenant": tenant, "disabled": true }))
}

pub async fn tenant_enable(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let tenant = arg_str(args, "tenant")?;
    let changed = state.db.set_tenant_disabled(tenant.clone(), false).await?;
    if !changed {
        return Err(AppError::ToolNotFound(format!("tenant {tenant}")));
    }
    Ok(json!({ "tenant": tenant, "disabled": false }))
}

pub async fn usage(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let window = arg_str_opt(args, "window").unwrap_or_else(|| "24h".to_string());
    let secs = crate::state::parse_window_secs(&window);
    let rows = state.db.usage_by_tenant_and_tool(secs).await?;
    let usage: Vec<Value> = rows
        .into_iter()
        .map(|u| {
            json!({
                "tenant": u.namespace,
                "tool": u.tool_name,
                "calls": u.stats.calls,
                "errors": u.stats.errors,
                "p50_ms": u.stats.p50_ms,
                "p95_ms": u.stats.p95_ms,
            })
        })
        .collect();
    Ok(json!({ "window": window, "usage": usage }))
}

pub async fn tool_list(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let tenant_ns = arg_str(args, "tenant")?;
    let tenant = state
        .db
        .find_tenant_by_namespace(tenant_ns.clone())
        .await?
        .ok_or_else(|| AppError::ToolNotFound(format!("tenant {tenant_ns}")))?;
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
    Ok(json!({ "tenant": tenant_ns, "tools": tools }))
}
