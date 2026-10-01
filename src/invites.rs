//! PRD-mcphost-invite-links: `host.invite.create`/`list`/`revoke`, the
//! `/i/{code}/mcp` first-call resolution (tenant creation + contact +
//! share, mirroring `control::signup` for the parts this PRD's own
//! Depends-on, PRD-mcphost-implicit-signup, has not landed yet to
//! provide), every tenant's standing invite, and the lineage/usage reads
//! `host.agent.lookup`/`host.usage` surface.
//!
//! Same module shape as `sharing.rs`/`consent.rs`: pure `AppState` +
//! arguments in, `serde_json::Value` (or [`AppError`]) out --
//! `handler.rs` is the only place that touches `rmcp` wire types.

use serde_json::{Value, json};

use crate::db::{InviteListRow, InviteRow, Tenant};
use crate::errors::AppError;
use crate::state::AppState;

/// requirement 3/11: the per-code invite-creation-event limiter -- "at
/// most 20 creations per code per hour" -- shared by both invite kinds
/// (AC6 for a `"created"` invite, AC10 for the `"standing"` one).
const INVITE_CREATIONS_PER_HOUR: i64 = 20;
/// requirement 11: the additional per-inviter-per-day cap on
/// `"standing"`-invite creation events only (a `"created"` invite's own
/// `max_uses` already bounds it).
const STANDING_INVITE_CREATIONS_PER_INVITER_PER_DAY: i64 = 100;
/// requirement 1: `max_uses`/`expires_in_days` defaults and caps.
const DEFAULT_MAX_USES: i64 = 10;
const MAX_MAX_USES: i64 = 100;
const DEFAULT_EXPIRES_IN_DAYS: i64 = 30;
const MAX_EXPIRES_IN_DAYS: i64 = 365;

fn arg_str(args: &Value, name: &str) -> Result<String, AppError> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppError::InvalidArgs(format!("missing required argument '{name}'")))
}

/// requirement 1: `host.invite.create`'s own `share` names must be the
/// inviter's own tools -- any visibility, not just already-shared ones
/// (requirement 2's claim-time grant promotes a private tool, see
/// [`apply_share_grants`]). Omitted entirely: defaults to every tool this
/// tenant has already shared with `visibility: "group"` (the TL;DR's own
/// wording) -- a `"public"` tool is never defaulted in, since it needs no
/// invite to reach.
async fn resolve_share_arg(
    state: &AppState,
    tenant: &Tenant,
    args: &Value,
) -> Result<Vec<String>, AppError> {
    match args.get("share") {
        None | Some(Value::Null) => {
            let tools = state.db.list_tools(tenant.id).await?;
            Ok(tools
                .into_iter()
                .filter(|t| t.visibility == "group")
                .map(|t| t.name)
                .collect())
        }
        Some(Value::Array(items)) => {
            let mut names = Vec::with_capacity(items.len());
            for item in items {
                let name = item
                    .as_str()
                    .ok_or_else(|| {
                        AppError::InvalidArgs("share: every entry must be a string".into())
                    })?
                    .to_string();
                if state.db.get_tool(tenant.id, name.clone()).await?.is_none() {
                    return Err(AppError::ToolNotFound(name));
                }
                names.push(name);
            }
            Ok(names)
        }
        Some(_) => Err(AppError::InvalidArgs(
            "share: must be an array of tool names".into(),
        )),
    }
}

fn invite_url(state: &AppState, code: &str) -> String {
    format!("{}/i/{}/mcp", state.public_url.trim_end_matches('/'), code)
}

fn invite_json(row: &InviteRow, invitees: &[String]) -> Value {
    json!({
        "kind": row.kind,
        "uses": row.uses,
        "max_uses": row.max_uses,
        "expires_at": row.expires_unix.map(crate::state::rfc3339_from_unix),
        "revoked": row.revoked_at.is_some(),
        "share": row.share,
        "caller_limit": row.caller_limit,
        "channel": row.channel,
        "invitees": invitees,
    })
}

/// `host.invite.create {share?, max_uses?, expires_in_days?,
/// caller_limit_per_day?}` (requirement 1, AC1, AC7).
pub async fn create(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let plan = state.plans.get(&tenant.plan).ok_or_else(|| {
        AppError::Internal(format!("tenant plan '{}' not in catalog", tenant.plan))
    })?;
    let live = state.db.count_live_created_invites(tenant.id).await?;
    if live >= plan.invites_max {
        return Err(AppError::invites_max(plan.invites_max));
    }

    let share = resolve_share_arg(state, tenant, args).await?;
    let max_uses = args
        .get("max_uses")
        .and_then(Value::as_i64)
        .unwrap_or(DEFAULT_MAX_USES)
        .clamp(1, MAX_MAX_USES);
    let expires_in_days = args
        .get("expires_in_days")
        .and_then(Value::as_i64)
        .unwrap_or(DEFAULT_EXPIRES_IN_DAYS)
        .clamp(1, MAX_EXPIRES_IN_DAYS);
    let caller_limit = args.get("caller_limit_per_day").and_then(Value::as_i64);

    let code = crate::auth::generate_invite_code();
    let code_hash = crate::auth::hash_key(&code);
    let expires_unix = crate::state::now_unix() + expires_in_days * 86_400;
    state
        .db
        .create_invite(
            code_hash,
            tenant.id,
            "created",
            share.clone(),
            Some(max_uses),
            caller_limit,
            None,
            Some(expires_unix),
        )
        .await?;

    Ok(json!({
        "code": code,
        "url": invite_url(state, &code),
        "max_uses": max_uses,
        "expires_at": crate::state::rfc3339_from_unix(expires_unix),
        "share": share,
        "uses": 0,
    }))
}

/// `host.invite.list()` (requirement 5, AC1).
pub async fn list(state: &AppState, tenant: &Tenant) -> Result<Value, AppError> {
    let rows: Vec<InviteListRow> = state.db.list_invites(tenant.id).await?;
    let invites: Vec<Value> = rows
        .iter()
        .map(|r| invite_json(&r.invite, &r.invitees))
        .collect();
    Ok(json!({ "invites": invites }))
}

/// `host.invite.revoke {code}` (requirement 5/11, AC1, AC10): a
/// `"created"` invite is revoked outright (existing invitees' shares and
/// contacts are untouched -- there is nothing here that undoes them);
/// the tenant's own `"standing"` invite is ROTATED instead (a fresh code,
/// same row) rather than revoked, since a tenant is meant to always have
/// exactly one.
pub async fn revoke(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let code = arg_str(args, "code")?;
    let code_hash = crate::auth::hash_key(&code);
    let invite = state
        .db
        .find_invite_by_code_hash(code_hash)
        .await?
        .ok_or_else(AppError::invite_not_found)?;
    if invite.inviter_tenant_id != tenant.id {
        return Err(AppError::invite_not_found());
    }
    if invite.kind == "standing" {
        let new_code = crate::auth::generate_invite_code();
        state
            .db
            .rotate_standing_invite(
                invite.id,
                crate::auth::hash_key(&new_code),
                new_code.clone(),
            )
            .await?;
        return Ok(json!({
            "kind": "standing",
            "url": invite_url(state, &new_code),
            "rotated": true,
        }));
    }
    state.db.revoke_invite(invite.id).await?;
    Ok(json!({ "kind": invite.kind, "revoked": true, "uses": invite.uses }))
}

/// requirement 10: every tenant's one standing invite, minted eagerly at
/// birth by [`claim_on_first_call`] (invite-created tenants) or lazily by
/// `control::whoami` (every other creation path -- `signup`, `url-page`,
/// and eventually `implicit`, none of which this PRD's own branch may
/// touch -- see this module's own doc comment). `find_or_create_standing_
/// invite`'s `INSERT ... WHERE NOT EXISTS` makes a concurrent double-call
/// (two `host.whoami`s racing on a tenant's very first call) safe; on
/// that race the loser's freshly-generated code is simply discarded and
/// the winner's row (with ITS code) is returned to both callers.
pub async fn ensure_standing_invite(
    state: &AppState,
    tenant_id: i64,
) -> Result<InviteRow, AppError> {
    let code = crate::auth::generate_invite_code();
    let code_hash = crate::auth::hash_key(&code);
    state
        .db
        .find_or_create_standing_invite(tenant_id, code, code_hash)
        .await
}

/// `host.whoami`'s `invite_url` (AC9): resolves (creating if needed) a
/// tenant's standing invite and returns its current URL in the clear.
/// Unlike the personal `/u/{secret}/mcp` URL (plaintext only on a
/// URL-bound or just-created session) a standing invite is meant to be
/// handed out repeatedly, so its plaintext is stored at rest (`invites.
/// code_plain`, see the migration's own doc comment) and this can return
/// it on every call, not just the one where it was minted.
pub async fn standing_invite_url(state: &AppState, tenant_id: i64) -> Result<String, AppError> {
    let row = ensure_standing_invite(state, tenant_id).await?;
    let code = row.code_plain.ok_or_else(AppError::invite_invalid)?;
    Ok(invite_url(state, &code))
}

/// requirement 2/4/6/10 (AC2, AC4, AC6): resolves a `/i/{code}/mcp`
/// request's first `host.*`/`billing.*` call with no credential at all --
/// creates the invitee tenant (mirroring `control::signup`'s own
/// creation internals, since PRD-mcphost-implicit-signup has not landed a
/// shared entry point yet -- see this module's doc comment), applies the
/// invite's grants, and returns a `signup`-shaped `Value` (carrying
/// `key`) so `handler::bind_session_to_created_tenant` can bind the
/// session through the EXACT same mechanism `signup`/`host.redeem`
/// already use, unmodified.
#[allow(clippy::too_many_arguments)]
pub async fn claim_on_first_call(
    state: &AppState,
    code: &str,
    source_ip: &str,
    synthetic_header: Option<&str>,
    client_name: Option<&str>,
    client_version: Option<&str>,
) -> Result<Value, AppError> {
    crate::bans::enforce(state, "addr", source_ip).await?;
    if let Some(pause) = state.signup_pause.status() {
        return Err(AppError::signup_paused(
            pause.message,
            pause.retry_after_secs,
        ));
    }
    if !state.disk_guard.is_ok(state.db.data_dir()) {
        return Err(AppError::disk_floor(
            state.disk_guard.free_bytes(state.db.data_dir()),
            state.disk_guard.floor_bytes(),
        ));
    }

    let code_hash = crate::auth::hash_key(code);
    let invite = state
        .db
        .find_invite_by_code_hash(code_hash)
        .await?
        .ok_or_else(AppError::invite_invalid)?;
    let now = crate::state::now_unix();
    if !invite.is_live(now) {
        return Err(AppError::invite_invalid());
    }

    // requirement 3/11: per-code creation-event limiter (20/hour),
    // regardless of kind -- AC6 (created) and AC10 (standing) alike.
    let since_hour = now - 3600;
    if !state
        .db
        .try_admit_invite_creation(invite.id, since_hour, INVITE_CREATIONS_PER_HOUR)
        .await?
    {
        return Err(AppError::invite_rate_limited(3600));
    }
    if invite.kind == "standing" {
        let since_day = now - 86_400;
        let count = state
            .db
            .count_invite_creations_for_inviter_since(invite.inviter_tenant_id, since_day)
            .await?;
        if count > STANDING_INVITE_CREATIONS_PER_INVITER_PER_DAY {
            return Err(AppError::invite_rate_limited(86_400));
        }
    }

    // requirement 2/4 (AC4): the one atomic admission -- a race for the
    // last slot can never create more than `max_uses` tenants.
    if !state.db.try_claim_invite_use(invite.id).await? {
        return Err(AppError::invite_invalid());
    }

    let Some(inviter) = state.db.find_tenant_by_id(invite.inviter_tenant_id).await? else {
        return Err(AppError::invite_invalid());
    };

    // ---- create the invitee tenant --------------------------------
    let display_name = format!("agent-{}", crate::state::new_ulid());
    let explicit_label = crate::control::validate_synthetic_header(synthetic_header);
    let harness_marker_present = synthetic_header.is_some();
    let class = crate::state::classify_source_class(
        source_ip,
        harness_marker_present,
        &display_name,
        client_name,
        &state.fleet_ips,
    );
    let fleet_ip_only_match = class == crate::state::SourceClass::Fleet
        && !crate::state::is_known_fleet_display_name(&display_name)
        && !client_name.is_some_and(crate::state::is_known_synthorg_client);
    let synthetic = if class.is_synthetic() {
        Some(explicit_label.unwrap_or_else(|| {
            if fleet_ip_only_match {
                "harness:fleet-ip".to_string()
            } else {
                "harness:unstamped".to_string()
            }
        }))
    } else {
        None
    };
    let (origin, origin_detail) = crate::state::derive_origin(class, synthetic.as_deref());

    let key = crate::auth::generate_key();
    let namespace = crate::auth::generate_namespace();
    let key_hash = crate::auth::hash_key(&key);
    let invited_by = format!("invite:{code}");
    let tenant = state
        .db
        .create_tenant_attributed_with_source(
            display_name,
            namespace.clone(),
            key_hash,
            synthetic,
            Some(class.as_str().to_string()),
            client_name.map(str::to_string),
            client_version.map(str::to_string),
            origin.to_string(),
            origin_detail,
            Some(invited_by),
        )
        .await?;
    state
        .db
        .set_tenant_invited_by(tenant.id, inviter.namespace.clone(), invite.id)
        .await?;

    // requirement 10: every invite-created tenant is born with its own
    // standing invite, same as any other creation path; its URL closes
    // the loop the onboarding note below names ("bring another agent").
    let own_invite_url = standing_invite_url(state, tenant.id).await.ok();

    // requirement 1/2's URL-bound counterpart (PRD-mcphost-url-bound-
    // tenants): mint the invitee's own personal URL eagerly, same as
    // `/u/new`'s own first-ever generation, so `onboarding.url` is usable
    // immediately rather than requiring a second `host.whoami` round
    // trip.
    let url_secret = crate::auth::generate_url_secret();
    let _ = state
        .db
        .rotate_tenant_url_secret(tenant.id, crate::auth::hash_key(&url_secret))
        .await;
    let onboarding_url = format!(
        "{}/u/{}/mcp",
        state.public_url.trim_end_matches('/'),
        url_secret
    );

    let claim_url = crate::claim::issue_claim_token(state, &tenant).await.ok();

    // requirement 2: contact accepted both ways, in the same step as
    // tenant creation -- `Db::insert_accepted_contact_pair` is the
    // system-level counterpart to the ordinary `contact_request`/
    // `contact_accept` dance (see its own doc comment).
    let via = format!("invite:{code}");
    state
        .db
        .insert_accepted_contact_pair(inviter.id, tenant.id, via)
        .await?;

    // requirement 2: apply every listed share, through the same
    // `host.tool_share`/`host.share.caller_limit` functions a manual
    // share would use.
    let shared_tools = apply_share_grants(state, &inviter, &tenant, &invite).await;

    let invite_line = own_invite_url
        .as_deref()
        .map(|u| format!(" To bring another agent, give it {u}."))
        .unwrap_or_default();
    let mut result = json!({
        "tenant": tenant.namespace,
        "tenant_id": tenant.namespace,
        "key": key,
        "namespace_verified": false,
        "onboarding": {
            "tenant": tenant.namespace,
            "url": onboarding_url,
            "invite_url": own_invite_url,
            "invited_by": inviter.namespace,
            "shared_tools": shared_tools,
            "note": format!(
                "You are now tenant {}. This invite made you {}'s contact and shared {} \
                 tool(s) with you. Save your URL ({onboarding_url}) as your mcphost server \
                 address -- it is your credential.{invite_line}",
                tenant.namespace,
                inviter.namespace,
                shared_tools.len(),
            ),
        },
    });
    if let Some(url) = claim_url
        && let Some(obj) = result.as_object_mut()
    {
        obj.insert("claim_url".to_string(), json!(url));
    }
    Ok(result)
}

/// requirement 2/13: promotes each listed tool to a visibility the
/// invitee can reach (a tool that's already `"public"` needs nothing; one
/// that's already `"group"` just gains the invitee as a member of its
/// EXISTING group, so other members are unaffected; a `"private"` tool is
/// promoted into a NEW group scoped to this one invite, `"invite-<id>"`,
/// so a one-hop invite chain (AC12) never grants a tool to anyone beyond
/// this specific invitee), then applies the invite's own `caller_limit`
/// (requirement 2) if it set one. Reuses `sharing::tool_share`/
/// `sharing::caller_limit` verbatim (technical considerations: "through
/// the same functions ... so quotas and audit rows are identical to
/// manual sharing") -- a tool name that no longer exists, or a
/// `shared_tools_max` quota the inviter has since exhausted, is skipped
/// rather than failing the whole claim (the invitee's tenant and contact
/// already exist by this point; a partial share is strictly better than
/// none).
async fn apply_share_grants(
    state: &AppState,
    inviter: &Tenant,
    invitee: &Tenant,
    invite: &InviteRow,
) -> Vec<String> {
    let mut applied = Vec::new();
    for name in &invite.share {
        let Ok(Some(row)) = state.db.get_tool(inviter.id, name.clone()).await else {
            continue;
        };
        let group_name = match row.visibility.as_str() {
            "public" => {
                applied.push(name.clone());
                continue;
            }
            "group" => row
                .shared_group
                .clone()
                .unwrap_or_else(|| format!("invite-{}", invite.id)),
            _ => format!("invite-{}", invite.id),
        };
        if row.visibility == "private" {
            let _ = state.db.create_group(inviter.id, group_name.clone()).await;
            let share_args = json!({
                "name": name,
                "visibility": "group",
                "group": group_name,
            });
            if crate::sharing::tool_share(state, inviter, &share_args)
                .await
                .is_err()
            {
                continue;
            }
        }
        if state
            .db
            .group_add_member(inviter.id, group_name, invitee.id)
            .await
            .unwrap_or(false)
            || row.visibility == "group"
            || row.visibility == "private"
        {
            applied.push(name.clone());
        }
        if let Some(calls_per_day) = invite.caller_limit {
            let limit_args = json!({
                "tool": name,
                "caller_tenant": invitee.namespace,
                "calls_per_day": calls_per_day,
            });
            let _ = crate::sharing::caller_limit(state, inviter, &limit_args).await;
        }
    }
    applied
}

/// `host.usage`'s `invites` block (requirement 12, AC11): `sent_7d`
/// (invite-creation events, either kind) and `accepted_7d` (tenants
/// actually created through one of this tenant's invites) in the
/// trailing 7 days, plus `k = accepted_7d / active_inviters_7d` -- for a
/// single tenant's own view, `active_inviters_7d` is 1 iff it sent at
/// least one invite in the window, else the ratio is reported as `0.0`
/// rather than dividing by zero.
pub async fn usage_block(state: &AppState, tenant: &Tenant) -> Result<Value, AppError> {
    let since = crate::state::now_unix() - 7 * 86_400;
    let (sent, accepted) = state
        .db
        .invite_funnel_since(tenant.id, tenant.namespace.clone(), since)
        .await?;
    let active_inviters = if sent > 0 { 1.0 } else { 0.0 };
    let k = if active_inviters > 0.0 {
        accepted as f64 / active_inviters
    } else {
        0.0
    };
    Ok(json!({
        "sent_7d": sent,
        "accepted_7d": accepted,
        "k": k,
    }))
}

/// PRD-mcphost-invite-links requirement 14 (AC13): `true` the first time
/// `session_id` asks (and remembers it for next time), `false` on every
/// later call for the same session -- the gate `handler::call_shared_tool`
/// uses to attach `_meta.invite_url` at most once per session.
pub fn mark_invite_hint_shown(state: &AppState, session_id: &str) -> bool {
    let mut sessions = state
        .invite_hint_sessions
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    sessions.insert(session_id.to_string())
}
