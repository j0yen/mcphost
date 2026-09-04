//! Business logic for the `admin.*` tools, visible only to `$MCPHOST_ADMIN_KEY`.

use serde_json::{Value, json};

use crate::db::TenantDeleteCounts;
use crate::errors::AppError;
use crate::state::AppState;

/// PRD-mcphost-tenant-delete requirement 6: a batch dry run or delete
/// touches at most this many tenants per call.
const MAX_PREFIX_BATCH: i64 = 500;
/// PRD-mcphost-tenant-delete requirement 2 / AC5: a prefix under this many
/// characters (empty included) is refused, so a typo cannot empty the box.
const MIN_PREFIX_LEN: usize = 4;

fn arg_str(args: &Value, name: &str) -> Result<String, AppError> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppError::InvalidArgs(format!("missing required argument '{name}'")))
}

fn arg_str_opt(args: &Value, name: &str) -> Option<String> {
    args.get(name).and_then(Value::as_str).map(str::to_string)
}

/// AC10 (P1 requirement 7): `admin.tenants(prefix?)` -- with no `prefix`,
/// the original unfiltered list; with one, the same `display_name`-prefix
/// filter the batch delete uses, minus its counts, so an operator can see
/// what a batch would target before running `tenant_delete_by_prefix`.
pub async fn tenants(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let prefix = arg_str_opt(args, "prefix");
    let rows = match prefix {
        Some(p) => state.db.list_tenants_by_prefix(p, None).await?.0,
        None => state.db.list_tenants().await?,
    };
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

fn counts_json(counts: &TenantDeleteCounts) -> Value {
    json!({
        "tools_removed": counts.tools_removed,
        "secrets_removed": counts.secrets_removed,
        "calls_removed": counts.calls_removed,
        "logs_removed": counts.logs_removed,
    })
}

/// P0 requirement 1 (AC1/AC2/AC7): delete one tenant and cascade every row
/// that references it, atomically, in `Db::delete_tenant`. Structured log
/// line + `admin_events` row (requirement 4 / AC8) happen here and in
/// `Db::delete_tenant` respectively -- the log is process-observability,
/// the DB row is the durable audit trail.
pub async fn tenant_delete(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let tenant = arg_str(args, "tenant")?;
    let (deleted, counts) = state
        .db
        .delete_tenant(tenant.clone())
        .await?
        .ok_or_else(|| AppError::TenantNotFound(tenant.clone()))?;
    tracing::info!(
        tenant = %deleted.namespace,
        action = "tenant_delete",
        dry_run = false,
        tools_removed = counts.tools_removed,
        secrets_removed = counts.secrets_removed,
        calls_removed = counts.calls_removed,
        logs_removed = counts.logs_removed,
        "admin tenant delete"
    );
    let mut result = json!({
        "tenant": deleted.namespace,
        "namespace": deleted.namespace,
    });
    if let Some(obj) = result.as_object_mut() {
        if let Some(counts_obj) = counts_json(&counts).as_object() {
            obj.extend(counts_obj.clone());
        }
    }
    Ok(result)
}

/// P0 requirement 2 (AC3/AC4/AC5/AC6): batch cascade-delete every tenant
/// whose `display_name` starts with `prefix`. `dry_run` (default true)
/// only previews counts; `dry_run=false` deletes one transaction per
/// tenant via the same `Db::delete_tenant` AC1 goes through, so a batch
/// delete's per-tenant behavior (cascade, audit row, log line) is
/// identical to a single delete, just looped.
pub async fn tenant_delete_by_prefix(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let prefix = arg_str(args, "prefix")?;
    let dry_run = args
        .get("dry_run")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    if prefix.chars().count() < MIN_PREFIX_LEN {
        return Err(AppError::InvalidParams(format!(
            "prefix must be at least {MIN_PREFIX_LEN} characters"
        )));
    }

    let (matches, truncated) = state
        .db
        .list_tenants_by_prefix(prefix.clone(), Some(MAX_PREFIX_BATCH))
        .await?;

    if dry_run {
        let mut items = Vec::with_capacity(matches.len());
        for t in &matches {
            let (_, counts) = state
                .db
                .tenant_delete_preview(t.namespace.clone())
                .await?
                .unwrap_or((t.clone(), TenantDeleteCounts::default()));
            let mut item = json!({
                "tenant": t.namespace,
                "display_name": t.display_name,
            });
            if let (Some(obj), Some(counts_obj)) =
                (item.as_object_mut(), counts_json(&counts).as_object())
            {
                obj.extend(counts_obj.clone());
            }
            items.push(item);
        }
        tracing::info!(
            prefix = %prefix,
            action = "tenant_delete_by_prefix",
            dry_run = true,
            matched = items.len(),
            truncated,
            "admin batch tenant delete (dry run)"
        );
        return Ok(json!({
            "dry_run": true,
            "matched": items,
            "truncated": truncated,
        }));
    }

    let mut total = TenantDeleteCounts::default();
    let mut deleted = Vec::with_capacity(matches.len());
    for t in &matches {
        if let Some((deleted_tenant, counts)) = state.db.delete_tenant(t.namespace.clone()).await? {
            total.accumulate(&counts);
            tracing::info!(
                tenant = %deleted_tenant.namespace,
                action = "tenant_delete_by_prefix",
                dry_run = false,
                tools_removed = counts.tools_removed,
                secrets_removed = counts.secrets_removed,
                calls_removed = counts.calls_removed,
                logs_removed = counts.logs_removed,
                "admin tenant delete (batch)"
            );
            deleted.push(deleted_tenant.namespace);
        }
    }
    let mut result = json!({
        "dry_run": false,
        "deleted": deleted,
        "truncated": truncated,
    });
    if let Some(obj) = result.as_object_mut()
        && let Some(counts_obj) = counts_json(&total).as_object()
    {
        obj.extend(counts_obj.clone());
    }
    Ok(result)
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

/// AC19: mark a tenant's domain namespace as verified so
/// `host.registry_publish` will run for it. The verification METHOD (DNS
/// or HTTP) is deliberately not this crate's concern (PRD Open Questions,
/// owned by Joe) -- this tool is the minimal admin-set boolean path the
/// PRD's registry-publish requirement can be implemented against without
/// deciding that question.
pub async fn tenant_verify_namespace(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let tenant = arg_str(args, "tenant")?;
    let domain_namespace = arg_str(args, "domain_namespace")?;
    let changed = state
        .db
        .set_tenant_namespace_verified(tenant.clone(), domain_namespace.clone())
        .await?;
    if !changed {
        return Err(AppError::ToolNotFound(format!("tenant {tenant}")));
    }
    Ok(json!({ "tenant": tenant, "domain_namespace": domain_namespace, "verified": true }))
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
