//! Business logic for the tenant-facing sharing surface: `host.tool_share`,
//! `host.tool_unshare`, `host.group.*`, `host.catalog.*`.
//! PRD-mcphost-sharing P0 requirements 1-6. Same shape as `control.rs`:
//! pure `AppState` + arguments in, `serde_json::Value` (or [`AppError`]) out
//! -- `handler.rs` is the only place that touches `rmcp` wire types.
//! `admin.shared_tools`/`admin.tool_unshare` live in `admin.rs` instead,
//! alongside the rest of the `admin.*` surface.

use serde_json::{Value, json};

use crate::db::{Tenant, ToolRow};
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

fn tool_json(namespace: &str, tool: &crate::db::ToolRow) -> Value {
    json!({
        "name": format!("{namespace}.{}", tool.name),
        "kind": tool.kind,
        "owner": namespace,
        "visibility": tool.visibility,
        "group": tool.shared_group,
        "description": tool.share_description,
    })
}

/// `host.tool_share(name, visibility, group?, description?)` (AC1, AC6):
/// `visibility` must be `"public"` or `"group"` (a tenant shares a tool
/// INTO one of those states; `host.tool_unshare` is the way back to
/// `"private"`, not this tool with `visibility: "private"`). `group` is
/// required when `visibility == "group"` and must already exist
/// (`host.group.create`).
pub async fn tool_share(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let name = arg_str(args, "name")?;
    let visibility = arg_str(args, "visibility")?;
    let description = arg_str_opt(args, "description");
    let group = arg_str_opt(args, "group");

    match visibility.as_str() {
        "public" => {}
        "group" => {
            let group_name = group
                .clone()
                .ok_or_else(|| AppError::InvalidArgs("visibility 'group' requires 'group'".into()))?;
            if state
                .db
                .list_groups(tenant.id)
                .await?
                .iter()
                .all(|(g, _)| g != &group_name)
            {
                return Err(AppError::GroupNotFound(group_name));
            }
        }
        other => {
            return Err(AppError::InvalidArgs(format!(
                "visibility must be 'public' or 'group', got '{other}'"
            )));
        }
    }

    // The tool must exist and already be this tenant's own.
    let row = state
        .db
        .get_tool(tenant.id, name.clone())
        .await?
        .ok_or_else(|| AppError::ToolNotFound(name.clone()))?;

    // AC6: re-sharing an already-shared tool (or changing its group) must
    // not count against the quota a second time -- only a private -> shared
    // transition does.
    if row.visibility == "private" {
        let plan = state.plans.get(&tenant.plan).ok_or_else(|| {
            AppError::Internal(format!(
                "tenant's plan '{}' is not in the loaded plan catalog",
                tenant.plan
            ))
        })?;
        let used = state.db.count_shared_tools(tenant.id).await?;
        if used >= plan.shared_tools_max {
            return Err(AppError::ShareQuotaExceeded {
                limit: plan.shared_tools_max,
                used,
            });
        }
    }

    state
        .db
        .set_tool_share(tenant.id, name.clone(), visibility.clone(), group.clone(), description.clone())
        .await?;

    Ok(json!({
        "name": format!("{}.{}", tenant.namespace, name),
        "visibility": visibility,
        "group": group,
        "description": description,
    }))
}

/// `host.tool_unshare(name)` (AC8): back to private. `false` (not found)
/// when `name` isn't one of this tenant's tools.
pub async fn tool_unshare(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let name = arg_str(args, "name")?;
    let removed = state.db.unshare_tool(tenant.id, name.clone()).await?;
    if !removed {
        return Err(AppError::ToolNotFound(name));
    }
    Ok(json!({ "name": name, "visibility": "private" }))
}

/// `host.group.create(name)` (requirement 1): idempotent.
pub async fn group_create(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let name = arg_str(args, "name")?;
    state.db.create_group(tenant.id, name.clone()).await?;
    Ok(json!({ "name": name, "created": true }))
}

/// `host.group.add(name, namespace)` (requirement 1): add `namespace` (a
/// tenant namespace, e.g. `t_cd34`) to this tenant's group `name`.
pub async fn group_add(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let name = arg_str(args, "name")?;
    let namespace = arg_str(args, "namespace")?;
    let member = state
        .db
        .find_tenant_by_namespace(namespace.clone())
        .await?
        .ok_or_else(|| AppError::TenantNotFound(namespace.clone()))?;
    let ok = state
        .db
        .group_add_member(tenant.id, name.clone(), member.id)
        .await?;
    if !ok {
        return Err(AppError::GroupNotFound(name));
    }
    Ok(json!({ "name": name, "namespace": namespace, "added": true }))
}

/// `host.group.remove(name, namespace)`: drop a member from this tenant's
/// group.
pub async fn group_remove(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let name = arg_str(args, "name")?;
    let namespace = arg_str(args, "namespace")?;
    let member = state
        .db
        .find_tenant_by_namespace(namespace.clone())
        .await?
        .ok_or_else(|| AppError::TenantNotFound(namespace.clone()))?;
    let ok = state
        .db
        .group_remove_member(tenant.id, name.clone(), member.id)
        .await?;
    if !ok {
        return Err(AppError::GroupNotFound(name));
    }
    Ok(json!({ "name": name, "namespace": namespace, "removed": true }))
}

/// `host.group.list()`: every group this tenant owns, with member
/// namespaces.
pub async fn group_list(state: &AppState, tenant: &Tenant) -> Result<Value, AppError> {
    let groups = state.db.list_groups(tenant.id).await?;
    let groups: Vec<Value> = groups
        .into_iter()
        .map(|(name, members)| json!({ "name": name, "members": members }))
        .collect();
    Ok(json!({ "groups": groups }))
}

/// PRD-mcphost-shared-tool-call-path requirement 1/2 (AC1-3, AC9): the
/// cross-tenant resolution primitive `handler.rs`'s raw `tools/call`
/// dispatch and `host.tool_call`'s qualified-name branch both call --
/// `owner_ns`'s tool `local_name`, if it exists AND is visible to `caller`
/// (`public`, or `group` with `caller` a member). Every miss reason (no
/// such namespace, no such tool, private, or caller not in the group)
/// collapses to the identical [`AppError::shared_tool_not_found`], so
/// neither the owner's existence nor the tool's is ever revealed to a
/// caller it isn't shared with.
pub async fn resolve_shared_tool(
    state: &AppState,
    caller: &Tenant,
    owner_ns: &str,
    local_name: &str,
) -> Result<(Tenant, ToolRow), AppError> {
    let not_found = || AppError::shared_tool_not_found(owner_ns, local_name);
    let (owner, row) = state
        .db
        .get_tool_by_owner_namespace(owner_ns.to_string(), local_name.to_string())
        .await?
        .ok_or_else(not_found)?;

    let visible = match row.visibility.as_str() {
        "public" => true,
        "group" => match &row.shared_group {
            Some(group) => state.db.is_group_member(owner.id, group.clone(), caller.id).await?,
            None => false,
        },
        _ => false,
    };
    if !visible {
        return Err(not_found());
    }
    Ok((owner, row))
}

/// `host.catalog.search(q?, limit?)` (AC7): a `LIKE` over name/description
/// among every `visibility = 'public'` tool, regardless of caller.
pub async fn catalog_search(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let q = arg_str_opt(args, "q");
    let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(20).clamp(1, 200);
    let rows = state.db.search_catalog(q, limit).await?;
    let tools: Vec<Value> = rows.iter().map(|(ns, t)| tool_json(ns, t)).collect();
    Ok(json!({ "tools": tools }))
}

/// `host.catalog.get(full_name)` (AC7): `full_name` is `<namespace>.<name>`;
/// 404-shaped `tool_not_found` (never revealing whether a private tool of
/// that name exists -- same non-leaking contract as the cross-tenant call
/// path) for anything that isn't a currently-public tool.
pub async fn catalog_get(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let full_name = arg_str(args, "full_name")?;
    let (namespace, local_name) = full_name
        .split_once('.')
        .ok_or_else(|| AppError::ToolNotFound(full_name.clone()))?;
    let tool = state
        .db
        .get_public_tool(namespace.to_string(), local_name.to_string())
        .await?
        .ok_or_else(|| AppError::ToolNotFound(full_name.clone()))?;
    let kind = state.kinds.get(&tool.kind);
    let args_schema = kind.map(|k| k.describe(&tool.spec).input_schema);
    let mut value = tool_json(namespace, &tool);
    if let Some(schema) = args_schema {
        value["args_schema"] = schema;
    }
    Ok(value)
}

/// The `/.well-known/mcp/catalog.json` document (AC7): the same public
/// listing as `host.catalog.search` with no query, for crawlers.
pub async fn catalog_document(state: &AppState) -> Result<Value, AppError> {
    let rows = state.db.search_catalog(None, 1000).await?;
    let tools: Vec<Value> = rows.iter().map(|(ns, t)| tool_json(ns, t)).collect();
    Ok(json!({ "tools": tools }))
}

/// PRD-mcphost-shared-tool-caller-usage requirement 3 (AC3): `quota_caller`
/// -- distinct from [`crate::billing::quota_exceeded`]'s `quota_exceeded`
/// (the plan-wide `calls_per_day` knob) since this is an owner-set cap on
/// one caller, on one shared tool, not a plan limit; `reset_at` (next UTC
/// midnight, same boundary [`crate::handler::McpHostHandler::check_calls_quota`]'s
/// own `resets_at` uses) tells the caller when to retry rather than
/// leaving it to guess.
pub fn quota_caller(caller_namespace: &str, tool: &str, limit: i64, used: i64, reset_at: String) -> AppError {
    AppError::Structured {
        code: "quota_caller",
        message: format!(
            "caller '{caller_namespace}' exceeded its calls_per_day limit ({limit}) for tool '{tool}'"
        ),
        data: json!({
            "caller_tenant": caller_namespace,
            "tool": tool,
            "limit": limit,
            "used": used,
            "reset_at": reset_at,
        }),
    }
}

/// `host.share.caller_limit {tool, caller_tenant, calls_per_day}` (AC3):
/// caps how many successful calls `caller_tenant` may make into this
/// tenant's shared `tool` per UTC day, enforced by
/// `McpHostHandler::call_published_tool` before dispatch. `tool` must
/// already be shared (a caller-limit on a tool nobody outside this tenant
/// can reach would be dead configuration) and `caller_tenant` must already
/// exist. Idempotent: calling again with a different `calls_per_day`
/// updates the existing row rather than erroring.
pub async fn caller_limit(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let name = arg_str(args, "tool")?;
    let caller_tenant = arg_str(args, "caller_tenant")?;
    let calls_per_day = args
        .get("calls_per_day")
        .and_then(Value::as_i64)
        .ok_or_else(|| AppError::InvalidArgs("missing required argument 'calls_per_day'".into()))?;
    if calls_per_day <= 0 {
        return Err(AppError::InvalidArgs("calls_per_day must be a positive integer".into()));
    }

    let row = state
        .db
        .get_tool(tenant.id, name.clone())
        .await?
        .ok_or_else(|| AppError::ToolNotFound(name.clone()))?;
    if row.visibility == "private" {
        return Err(AppError::InvalidArgs(format!(
            "tool '{name}' must be shared (host.tool_share) before setting a caller_limit"
        )));
    }
    let caller = state
        .db
        .find_tenant_by_namespace(caller_tenant.clone())
        .await?
        .ok_or_else(|| AppError::TenantNotFound(caller_tenant.clone()))?;

    state
        .db
        .set_caller_limit(tenant.id, name.clone(), caller.id, calls_per_day)
        .await?;
    Ok(json!({
        "tool": name,
        "caller_tenant": caller_tenant,
        "calls_per_day": calls_per_day,
    }))
}

/// `host.share.caller_limit_remove {tool, caller_tenant}`: drop a limit set
/// by [`caller_limit`]. `removed: false` (not an error) when no such limit
/// existed -- same "idempotent, report what happened" convention
/// `host.tool_unshare`'s own `false` case documents.
pub async fn caller_limit_remove(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let name = arg_str(args, "tool")?;
    let caller_tenant = arg_str(args, "caller_tenant")?;
    let caller = state
        .db
        .find_tenant_by_namespace(caller_tenant.clone())
        .await?
        .ok_or_else(|| AppError::TenantNotFound(caller_tenant.clone()))?;
    let removed = state
        .db
        .remove_caller_limit(tenant.id, name.clone(), caller.id)
        .await?;
    Ok(json!({ "tool": name, "caller_tenant": caller_tenant, "removed": removed }))
}
