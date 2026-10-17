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

/// PRD-mcphost-scoped-tool-share R1: what a share lets its grantee do. The
/// ONE source of the scope set: `host.tool_share`'s `scopes.items.enum`,
/// `_meta.share_scopes`, the `host.catalog.get` `scopes` field and the
/// llms.txt line all render [`ShareScope::ALL`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareScope {
    /// Call the shared tool.
    Call,
    /// Read the (redacted) spec through `host.tool_spec_shared`.
    ReadSource,
    /// Copy the tool into the grantee's own tenant with `host.tool_fork`.
    Fork,
}

impl ShareScope {
    pub const ALL: [ShareScope; 3] = [ShareScope::Call, ShareScope::ReadSource, ShareScope::Fork];

    pub fn as_str(self) -> &'static str {
        match self {
            ShareScope::Call => "call",
            ShareScope::ReadSource => "read_source",
            ShareScope::Fork => "fork",
        }
    }

    pub fn parse(name: &str) -> Option<ShareScope> {
        Self::ALL.into_iter().find(|s| s.as_str() == name)
    }

    /// The wire names of [`Self::ALL`], in order.
    pub fn names() -> Vec<&'static str> {
        Self::ALL.iter().map(|s| s.as_str()).collect()
    }

    /// Scopes a share with no recorded set grants: what every share predating
    /// the scopes column had (migration 0120's backfill).
    pub fn legacy_default() -> Vec<String> {
        vec!["call".to_string(), "read_source".to_string()]
    }

    /// Does `granted` allow `self`? `fork` implies `read_source` (a copy
    /// carries the source).
    pub fn granted_in(self, granted: &[String]) -> bool {
        let has = |s: ShareScope| granted.iter().any(|g| g == s.as_str());
        has(self) || (self == ShareScope::ReadSource && has(ShareScope::Fork))
    }
}

/// R3: `scope_missing {required, granted}`.
pub fn scope_missing(required: ShareScope, granted: &[String]) -> AppError {
    AppError::Structured {
        code: "scope_missing",
        message: format!(
            "this share does not grant '{}' (granted: {})",
            required.as_str(),
            granted.join(", ")
        ),
        data: json!({"required": required.as_str(), "granted": granted}),
    }
}

/// R5: `share_revoked {tool, revoked_at}`.
pub fn share_revoked(tool: &str, revoked_at: i64) -> AppError {
    AppError::Structured {
        code: "share_revoked",
        message: format!("the owner revoked the share of '{tool}'"),
        data: json!({"tool": tool, "revoked_at": crate::state::rfc3339_from_unix(revoked_at)}),
    }
}

fn arg_str(args: &Value, name: &str) -> Result<String, AppError> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppError::InvalidArgs(format!("missing required argument '{name}'")))
}

fn arg_str_opt(args: &Value, name: &str) -> Option<String> {
    args.get(name).and_then(Value::as_str).map(str::to_string)
}

/// PRD-mcphost-publish-visibility-and-tags R1: the one set of visibility
/// names. `tools.visibility`, the `host.tool_publish` / `host.tool_share`
/// schemas' `enum`s and the validators below all render from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Private,
    Unlisted,
    Group,
    Public,
    Url,
}

impl Visibility {
    pub const ALL: [Visibility; 5] =
        [Visibility::Private, Visibility::Unlisted, Visibility::Group, Visibility::Public, Visibility::Url];
    /// What `host.tool_publish` accepts.
    pub const PUBLISHABLE: [Visibility; 3] = [Visibility::Private, Visibility::Unlisted, Visibility::Public];
    /// What `host.tool_share` accepts (`host.tool_unshare` is the way back
    /// to `private`).
    pub const SHAREABLE: [Visibility; 4] =
        [Visibility::Unlisted, Visibility::Group, Visibility::Public, Visibility::Url];

    pub fn as_str(self) -> &'static str {
        match self {
            Visibility::Private => "private",
            Visibility::Unlisted => "unlisted",
            Visibility::Group => "group",
            Visibility::Public => "public",
            Visibility::Url => "url",
        }
    }

    pub fn parse(s: &str) -> Option<Visibility> {
        Self::ALL.into_iter().find(|v| v.as_str() == s)
    }

    /// The JSON-schema `enum` array for a slice of variants.
    pub fn schema_enum(set: &[Visibility]) -> Value {
        json!(set.iter().map(|v| v.as_str()).collect::<Vec<_>>())
    }

    fn names(set: &[Visibility]) -> String {
        set.iter().map(|v| format!("'{}'", v.as_str())).collect::<Vec<_>>().join(", ")
    }

    /// `visibility` as `host.tool_publish` accepts it.
    pub fn parse_publishable(s: &str) -> Result<Visibility, AppError> {
        Self::parse(s)
            .filter(|v| Self::PUBLISHABLE.contains(v))
            .ok_or_else(|| {
                AppError::InvalidArgs(format!(
                    "visibility must be one of {}, got '{s}'",
                    Self::names(&Self::PUBLISHABLE)
                ))
            })
    }

    /// `visibility` as `host.tool_share` accepts it.
    pub fn parse_shareable(s: &str) -> Result<Visibility, AppError> {
        Self::parse(s)
            .filter(|v| Self::SHAREABLE.contains(v))
            .ok_or_else(|| {
                AppError::InvalidArgs(format!(
                    "visibility must be one of {}, got '{s}'",
                    Self::names(&Self::SHAREABLE)
                ))
            })
    }
}

pub const MAX_TAGS: usize = 16;

/// R2: `tags` -- at most [`MAX_TAGS`], each `^[a-z0-9][a-z0-9-]{0,31}$` after
/// lower-casing; deduped, order kept. `Ok(None)` when the argument is absent.
pub fn parse_tags(args: &Value) -> Result<Option<Vec<String>>, AppError> {
    let raw = match args.get("tags") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Array(list)) => list,
        Some(_) => return Err(AppError::InvalidArgs("tags must be an array of strings".into())),
    };
    let mut out: Vec<String> = Vec::new();
    for item in raw {
        let tag = item
            .as_str()
            .ok_or_else(|| AppError::InvalidArgs("tags must be an array of strings".into()))?
            .to_lowercase();
        let valid = !tag.is_empty()
            && tag.len() <= 32
            && tag.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
            && tag.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !valid {
            return Err(AppError::InvalidArgs(format!(
                "tag '{tag}' is invalid: use 1-32 chars of a-z, 0-9 and '-', starting with a letter or digit"
            )));
        }
        if !out.contains(&tag) {
            out.push(tag);
        }
    }
    if out.len() > MAX_TAGS {
        return Err(AppError::InvalidArgs(format!("tags: at most {MAX_TAGS} tags per tool, got {}", out.len())));
    }
    Ok(Some(out))
}

fn tool_json(namespace: &str, tool: &crate::db::ToolRow) -> Value {
    json!({
        "name": format!("{namespace}.{}", tool.name),
        "kind": tool.kind,
        "owner": namespace,
        "visibility": tool.visibility,
        "group": tool.shared_group,
        "description": tool.share_description,
        "tags": tool.tags,
        "tested_version": tool.tested_version,
        "stale_test": tool.tested_version != Some(tool.current_version),
    })
}

/// R2: the `scopes` argument. Absent: `["call"]`, or `["call","read_source"]`
/// for an `expose_spec: true` share (that flag already granted the readback).
/// `url` shares allow only `call`.
fn parse_scopes(args: &Value, visibility: &str, expose_spec: bool) -> Result<Vec<String>, AppError> {
    let valid = ShareScope::names().join(", ");
    let Some(raw) = args.get("scopes") else {
        return Ok(if expose_spec { ShareScope::legacy_default() } else { vec!["call".to_string()] });
    };
    let items = raw
        .as_array()
        .filter(|a| !a.is_empty())
        .ok_or_else(|| AppError::InvalidArgs(format!("scopes must be a non-empty array of: {valid}")))?;
    let mut picked = Vec::new();
    for item in items {
        let scope = item.as_str().and_then(ShareScope::parse).ok_or_else(|| {
            AppError::InvalidArgs(format!("unknown scope {item}; valid scopes are: {valid}"))
        })?;
        picked.push(scope);
    }
    if visibility == "url" && picked.iter().any(|s| *s != ShareScope::Call) {
        return Err(AppError::InvalidArgs("url shares allow only the 'call' scope".into()));
    }
    Ok(ShareScope::ALL
        .into_iter()
        .filter(|s| picked.contains(s))
        .map(|s| s.as_str().to_string())
        .collect())
}

/// R2: `rate_per_hour_per_caller`, 1..=10000, optional.
fn parse_rate(args: &Value) -> Result<Option<i64>, AppError> {
    match args.get("rate_per_hour_per_caller") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => match v.as_i64() {
            Some(n) if (1..=10_000).contains(&n) => Ok(Some(n)),
            _ => Err(AppError::InvalidArgs("rate_per_hour_per_caller must be an integer 1..10000".into())),
        },
    }
}

/// `host.tool_share(name, visibility, group?, description?)` (AC1, AC6):
/// `visibility` must be `"public"`, `"group"`, or (PRD-mcphost-public-tool-url
/// requirement 1) `"url"` (a tenant shares a tool INTO one of those
/// states; `host.tool_unshare` is the way back to `"private"`, not this
/// tool with `visibility: "private"`). `group` is required when
/// `visibility == "group"` and must already exist (`host.group.create`).
pub async fn tool_share(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let name = arg_str(args, "name")?;
    let visibility = arg_str(args, "visibility")?;
    let description = arg_str_opt(args, "description");
    let group = arg_str_opt(args, "group");
    // PRD-mcphost-shared-tool-spec-readback requirement 1: owner opt-in,
    // default closed.
    let expose_spec = args.get("expose_spec").and_then(Value::as_bool).unwrap_or(false);

    let scopes = parse_scopes(args, &visibility, expose_spec)?;
    let rate_per_hour = parse_rate(args)?;

    let vis = Visibility::parse_shareable(&visibility)?;
    let new_tags = parse_tags(args)?;
    if vis == Visibility::Group {
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

    // PRD-mcphost-public-tool-url requirement 1 (AC1): minted/reused BEFORE
    // `set_tool_share` flips `visibility`, so a storage failure here never
    // leaves the tool shared with no token to show for it. Requirement 8
    // (AC8): a tool that was previously `url`-shared and is now being
    // shared into `public`/`group` instead has its old token revoked --
    // switching visibility is not a no-op for an already-live public URL.
    let token = if visibility == "url" {
        Some(crate::public_tool::mint_or_reuse_url_share_token(state, tenant, &name).await?)
    } else {
        if row.visibility == "url" {
            crate::public_tool::revoke_url_share(state, tenant.id, &name).await?;
        }
        None
    };

    if let Some(new_tags) = new_tags {
        let mut merged = row.tags.clone();
        for t in new_tags {
            if !merged.contains(&t) {
                merged.push(t);
            }
        }
        if merged.len() > MAX_TAGS {
            return Err(AppError::InvalidArgs(format!(
                "tags: at most {MAX_TAGS} tags per tool, got {} after merging",
                merged.len()
            )));
        }
        state.db.set_tool_tags(tenant.id, name.clone(), merged).await?;
    }

    state
        .db
        .set_tool_share(
            tenant.id,
            name.clone(),
            visibility.clone(),
            group.clone(),
            description.clone(),
            expose_spec,
        )
        .await?;
    state
        .db
        .set_share_scopes(tenant.id, name.clone(), scopes.clone(), rate_per_hour)
        .await?;

    let after = state.db.get_tool(tenant.id, name.clone()).await?.unwrap_or(row.clone());
    // R2/R4: the tenants whose `tools/list` this share changes -- the new
    // group's members (the grantees) plus any previous group's members who
    // just lost the entry. `grantees_notified` counts the former, through
    // the same membership `shared_visibility` evaluates.
    let grantees = if visibility == "group" {
        group_grantee_ids(state, tenant, group.as_deref()).await?
    } else {
        Vec::new()
    };
    let mut touched = grantees.clone();
    if row.visibility == "group" && row.shared_group != group {
        touched.extend(group_grantee_ids(state, tenant, row.shared_group.as_deref()).await?);
    }
    state.db.touch_tool_change_for(touched).await?;

    let mut result = json!({
        "name": format!("{}.{}", tenant.namespace, name),
        "visibility": visibility,
        "tags": after.tags,
        "tested_version": after.tested_version,
        "stale_test": after.tested_version != Some(after.current_version),
        "group": group,
        "description": description,
        "expose_spec": expose_spec,
        "listed_for": {"visibility": visibility, "grantees_notified": grantees.len()},
        "scopes": scopes,
        "rate_per_hour_per_caller": rate_per_hour,
    });
    if let Some(token) = token
        && let Some(obj) = result.as_object_mut()
    {
        obj.insert("url".to_string(), json!(crate::public_tool::public_tool_url(state, &token, &name)));
        // PRD-mcphost-form-tool R2: the argument form, only for a tool
        // published with `ui.form`.
        if crate::form_page::has_form(&row.spec) {
            obj.insert("form_url".to_string(), json!(crate::form_page::form_url(state, &token, &name)));
        }
        // PRD-mcphost-demo-url-per-tool R4: the read-only demo page, next to
        // the callable `url`.
        obj.insert(
            "demo_url".to_string(),
            json!(crate::demo_page::demo_url(state, &tenant.namespace, &name)),
        );
    }
    Ok(result)
}

/// `host.tool_unshare(name)` (AC8): back to private. `false` (not found)
/// when `name` isn't one of this tenant's tools.
pub async fn tool_unshare(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let name = arg_str(args, "name")?;
    // R2/R6: former group grantees must see the entry go on their next list.
    let former = match state.db.get_tool(tenant.id, name.clone()).await? {
        Some(row) if row.visibility == "group" => {
            group_grantee_ids(state, tenant, row.shared_group.as_deref()).await?
        }
        _ => Vec::new(),
    };
    let removed = state.db.unshare_tool(tenant.id, name.clone()).await?;
    if !removed {
        return Err(AppError::ToolNotFound(name));
    }
    state.db.touch_tool_change_for(former).await?;
    // PRD-mcphost-public-tool-url requirement 8 (AC4/AC8): a no-op when
    // this tool never had a public URL token.
    crate::public_tool::revoke_url_share(state, tenant.id, &name).await?;
    Ok(json!({ "name": name, "visibility": "private" }))
}

/// PRD-mcphost-scoped-tool-share R6: `host.tool_fork(owner, tool, as)` copies a
/// shared tool's `kind` + `spec` into the caller's tenant as a private tool
/// named `as`. Needs the `fork` scope ([`resolve_shared_tool`]); counts toward
/// `tools_max`; records `forked_from` (`<owner>.<tool>`). The owner's `env`
/// map is dropped from the copy -- a fork never carries the owner's values.
pub async fn tool_fork(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let owner_ns = arg_str(args, "owner")?;
    let tool = arg_str(args, "tool")?;
    let new_name = arg_str(args, "as")?;
    crate::state::validate_tool_name(&new_name)?;

    let (_owner, row) = resolve_shared_tool(state, tenant, &owner_ns, &tool, ShareScope::Fork).await?;

    if state.db.get_tool(tenant.id, new_name.clone()).await?.is_some() {
        return Err(AppError::InvalidArgs(format!("a tool named '{new_name}' already exists in your tenant")));
    }
    let plan = state.plans.get(&tenant.plan).ok_or_else(|| {
        AppError::Internal(format!("tenant's plan '{}' is not in the loaded plan catalog", tenant.plan))
    })?;
    let count = state.db.count_tools(tenant.id).await?;
    let effective_max = plan.tools_max.min(crate::state::MAX_TOOLS_PER_TENANT);
    if count >= effective_max {
        return Err(crate::billing::quota_exceeded(state, &tenant.plan, "tools_max", effective_max, count, None));
    }

    let mut spec = row.spec.clone();
    if let Some(obj) = spec.as_object_mut() {
        obj.remove("env");
    }
    state
        .db
        .upsert_tool(tenant.id, new_name.clone(), row.kind.clone(), spec, plan.versions_max, Vec::new())
        .await?;
    let forked_from = format!("{owner_ns}.{tool}");
    state.db.set_forked_from(tenant.id, new_name.clone(), forked_from.clone()).await?;
    Ok(json!({
        "name": format!("{}.{new_name}", tenant.namespace),
        "kind": row.kind,
        "forked_from": forked_from,
        "visibility": "private",
    }))
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

/// Tenant ids that are members of `owner`'s group `group` (the owner itself
/// excluded) -- exactly the callers [`shared_visibility`] answers `"group"`
/// for when a tool is shared to that group.
async fn group_grantee_ids(state: &AppState, owner: &Tenant, group: Option<&str>) -> Result<Vec<i64>, AppError> {
    let Some(group) = group else { return Ok(Vec::new()) };
    let mut ids = state.db.group_member_ids(owner.id, group.to_string()).await?;
    ids.retain(|id| *id != owner.id);
    Ok(ids)
}

/// PRD-mcphost-shared-tools-in-grantee-list R1: the ONE visibility rule for
/// a tool owned by `owner` and seen by `caller` -- `Some("public")`,
/// `Some("group")` when `caller` is a member of the share's group, else
/// `None`. [`resolve_shared_tool`] (call side) and [`visible_shared_tools`]
/// (list side) both go through it, so list and call cannot drift.
pub async fn shared_visibility(
    state: &AppState,
    caller: &Tenant,
    owner: &Tenant,
    row: &ToolRow,
) -> Result<Option<&'static str>, AppError> {
    Ok(match row.visibility.as_str() {
        "public" => Some("public"),
        "unlisted" => Some("unlisted"),
        "group" => match &row.shared_group {
            Some(group) if state.db.is_group_member(owner.id, group.clone(), caller.id).await? => {
                Some("group")
            }
            _ => None,
        },
        _ => None,
    })
}

/// R1: every group-shared tool `caller` may call (public shares are
/// discoverable via `host.tool.list_shared`, never listed). Candidates come
/// from the share table; each is confirmed by [`shared_visibility`].
pub async fn visible_shared_tools(
    state: &AppState,
    caller: &Tenant,
) -> Result<Vec<(Tenant, ToolRow)>, AppError> {
    let candidates = state
        .db
        .shared_tools_for_caller(caller.id, caller.namespace.clone())
        .await?;
    let mut out = Vec::new();
    for (owner_ns, name, via) in candidates {
        if !via.starts_with("group:") {
            continue;
        }
        let Some((owner, row)) = state.db.get_tool_by_owner_namespace(owner_ns, name).await? else {
            continue;
        };
        if shared_visibility(state, caller, &owner, &row).await? == Some("group") {
            out.push((owner, row));
        }
    }
    Ok(out)
}

/// PRD-mcphost-shared-tools-in-grantee-list R3: `host.tool.list_shared
/// {q?, limit?}` -- public shares plus the group shares `caller` can call,
/// with the `_meta` fields `tools/list` uses for group entries.
pub async fn list_shared(state: &AppState, caller: &Tenant, args: &Value) -> Result<Value, AppError> {
    let q = arg_str_opt(args, "q");
    let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(50).clamp(1, 200) as usize;
    let needle = q.as_ref().map(|q| q.to_lowercase());
    let matches = |ns: &str, row: &ToolRow| match &needle {
        None => true,
        Some(n) => {
            format!("{ns}.{}", row.name).to_lowercase().contains(n)
                || row.share_description.as_deref().is_some_and(|d| d.to_lowercase().contains(n))
        }
    };
    let entry = |ns: &str, row: &ToolRow, vis: &str| {
        json!({
            "name": format!("{ns}.{}", row.name),
            "description": row.share_description,
            "_meta": {"kind": row.kind, "shared_by": ns, "shared_visibility": vis},
        })
    };
    let mut all: Vec<Value> = visible_shared_tools(state, caller)
        .await?
        .iter()
        .filter(|(o, r)| matches(&o.namespace, r))
        .map(|(o, r)| entry(&o.namespace, r, "group"))
        .collect();
    for (ns, row) in state.db.search_catalog(q.clone(), 1000).await? {
        if ns != caller.namespace {
            all.push(entry(&ns, &row, "public"));
        }
    }
    let truncated = all.len() > limit;
    all.truncate(limit);
    Ok(json!({ "tools": all, "truncated": truncated }))
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
    required: ShareScope,
) -> Result<(Tenant, ToolRow), AppError> {
    let (owner, row) = resolve_visible(state, caller, owner_ns, local_name).await?;
    let granted = state
        .db
        .get_share_meta(owner.id, local_name.to_string())
        .await?
        .and_then(|m| m.scopes)
        .unwrap_or_else(ShareScope::legacy_default);
    if !required.granted_in(&granted) {
        return Err(scope_missing(required, &granted));
    }
    Ok((owner, row))
}

/// The scope-free half of [`resolve_shared_tool`]: visible to `caller` (or the
/// not-found / `share_revoked` refusal).
async fn resolve_visible(
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

    if shared_visibility(state, caller, &owner, &row).await?.is_none() {
        // R5: a caller that ran this tool in the last 7 days learns the share
        // was revoked; anyone else gets the identical not-found. The
        // `current_key_id().is_none()` carve-out below reconciles two ACs:
        // - PRD-mcphost-key-scoped-to-tools-b AC2: a sub-key call after an
        //   unshare is a plain `not_found`;
        // - PRD-mcphost-scoped-tool-share R5 / shsc_ac04: a primary-key caller
        //   gets `share_revoked`.
        // Why: `share_revoked` must not leak share history across a sub-key
        // scope boundary.
        if row.visibility == "private"
            && crate::subkeys::current_key_id().is_none()
            && let Some(meta) = state.db.get_share_meta(owner.id, local_name.to_string()).await?
            && let Some(revoked_at) = meta.unshared_unix
        {
            let since = crate::state::now_unix() - REVOKED_WINDOW_SECS;
            if state
                .db
                .caller_called_since(owner.id, local_name.to_string(), caller.id, since)
                .await?
            {
                return Err(share_revoked(&format!("{owner_ns}.{local_name}"), revoked_at));
            }
        }
        return Err(not_found());
    }
    Ok((owner, row))
}

/// How long after an unshare a past caller still gets `share_revoked`.
const REVOKED_WINDOW_SECS: i64 = 7 * 86_400;

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
/// path) for anything that isn't a currently-public tool, or a group-shared
/// one `caller` is a member of. PRD-mcphost-scoped-tool-share R1: the entry
/// carries `scopes` (what the share grants); it never carries a `spec`.
pub async fn catalog_get(state: &AppState, caller: Option<&Tenant>, args: &Value) -> Result<Value, AppError> {
    let full_name = arg_str(args, "full_name")?;
    let (namespace, local_name) = full_name
        .split_once('.')
        .ok_or_else(|| AppError::ToolNotFound(full_name.clone()))?;
    let public = state
        .db
        .get_public_tool(namespace.to_string(), local_name.to_string())
        .await?;
    let tool = match (public, caller) {
        (Some(tool), _) => tool,
        (None, Some(caller)) => {
            resolve_visible(state, caller, namespace, local_name)
                .await
                .map(|(_, row)| row)
                .map_err(|_| AppError::ToolNotFound(full_name.clone()))?
        }
        (None, None) => return Err(AppError::ToolNotFound(full_name.clone())),
    };
    let kind = state.kinds.get(&tool.kind);
    let args_schema = kind.map(|k| k.describe(&tool.spec).input_schema);
    let mut value = tool_json(namespace, &tool);
    if let Some(schema) = args_schema {
        value["args_schema"] = schema;
    }
    let scopes = match state.db.find_tenant_by_namespace(namespace.to_string()).await? {
        Some(owner) => state.db.get_share_meta(owner.id, local_name.to_string()).await?,
        None => None,
    }
    .and_then(|m| m.scopes)
    .unwrap_or_else(ShareScope::legacy_default);
    value["scopes"] = json!(scopes);
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
