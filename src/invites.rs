//! Business logic for `host.invite.create`/`list`/`revoke`, the `/i/{code}/mcp`
//! redemption path, and the `_meta.invite_url` agent-facing hint.
//! PRD-mcphost-invite-links. Same shape as `sharing.rs`/`consent.rs`: pure
//! `AppState` + arguments in, `serde_json::Value` (or [`AppError`]) out --
//! `handler.rs` is the only place that touches `rmcp` wire types.
//!
//! This PRD's own `Depends-on: PRD-mcphost-implicit-signup` names a
//! "session behaves as anonymous and implicit-signs-up on first `host.*`
//! call" mechanism this crate does not otherwise have (there is no generic
//! anonymous-call-creates-a-tenant path anywhere else in `handler.rs`).
//! [`claim_on_first_call`] is that mechanism, built narrowly for this one
//! route rather than as a crate-wide feature: `handler::call_tool` calls it
//! only when a request on `/i/{code}/mcp` resolves to `Auth::Anonymous`
//! (no header, no `tenant_key`, no existing session binding), which by
//! construction can only ever be this session's very first call --
//! `Db::claim_invite` binds the session before returning, so every later
//! call on the same connection resolves through the ordinary session-
//! binding step instead and never reaches this function again
//! (requirement 6).
//!
//! A lazy "mint a standing invite for a pre-existing tenant on its first
//! `host.whoami`" path (requirement 10's other half) is deliberately not
//! built here -- no acceptance criterion in this build exercises a tenant
//! created before migration 0067, and every tenant this build's tests
//! create goes through `control::signup` or [`claim_on_first_call`], both
//! of which mint one at birth.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use crate::db::Tenant;
use crate::errors::AppError;
use crate::state::AppState;

fn arg_str(args: &Value, name: &str) -> Result<String, AppError> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppError::InvalidArgs(format!("missing required argument '{name}'")))
}

fn arg_str_array(args: &Value, name: &str) -> Vec<String> {
    args.get(name)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default()
}

/// Requirement 1: `max_uses` default 10, cap 100.
const DEFAULT_MAX_USES: i64 = 10;
const MAX_MAX_USES: i64 = 100;
/// Requirement 1: `expires_in_days` default 30, cap 365.
const DEFAULT_EXPIRES_IN_DAYS: i64 = 30;
const MAX_EXPIRES_IN_DAYS: i64 = 365;

pub(crate) fn invite_url(state: &AppState, code: &str) -> String {
    format!("{}/i/{code}/mcp", state.public_url.trim_end_matches('/'))
}

/// `host.invite.create {share?, max_uses?, expires_in_days?, caller_limit_per_day?}`
/// (requirement 1 / AC1): a capped, expiring, revocable link. `share`
/// names must already be this tenant's own tools (any other name is the
/// same [`AppError::ToolNotFound`] `host.tool_share` itself would raise).
pub async fn create(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let share = arg_str_array(args, "share");
    for name in &share {
        state
            .db
            .get_tool(tenant.id, name.clone())
            .await?
            .ok_or_else(|| AppError::ToolNotFound(name.clone()))?;
    }
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

    // Requirement 1 (AC7): the free plan allows 3 live standard invites;
    // a tenant's own standing invite never counts (`Db::count_live_standard_invites`
    // filters `kind = 'standard'`).
    let plan = state.plans.get(&tenant.plan).ok_or_else(|| {
        AppError::Internal(format!(
            "tenant's plan '{}' is not in the loaded plan catalog",
            tenant.plan
        ))
    })?;
    let used = state.db.count_live_standard_invites(tenant.id).await?;
    if used >= plan.invites_max {
        return Err(AppError::invites_quota_exceeded(plan.invites_max, used));
    }

    let code = crate::auth::generate_url_secret();
    let code_hash = crate::auth::hash_key(&code);
    let expires_unix = crate::state::now_unix() + expires_in_days * 86_400;
    let invite = state
        .db
        .create_invite(
            tenant.id,
            "standard",
            code_hash,
            None,
            share.clone(),
            Some(max_uses),
            Some(expires_unix),
            caller_limit,
        )
        .await?;

    let mut body = json!({
        "code": code,
        "url": invite_url(state, &code),
        "max_uses": invite.max_uses,
        "expires_at": invite.expires_unix,
        "share": share,
    });
    // PRD-mcphost-reachability-alt-host requirement 2 / goal "JSON
    // responses get alt_url/alt_endpoint fields": present only when
    // alternates are configured (AC5 keeps today's response byte-for-byte
    // otherwise -- no key added, not an absent-vs-null distinction).
    if let (Some(alt), Value::Object(map)) = (state.alt_public_urls.first(), &mut body) {
        map.insert(
            "alt_url".to_string(),
            json!(format!("{}/i/{code}/mcp", alt.trim_end_matches('/'))),
        );
    }
    Ok(body)
}

/// `host.invite.list` (requirement 5 / AC1, requirement 10 / AC9): every
/// invite this tenant owns, standard and standing alike. A standing
/// invite's entry omits `max_uses`/`expires_at`/`share` entirely (AC9: "no
/// expires_at, no max_uses") rather than nulling them, since it has none.
pub async fn list(state: &AppState, tenant: &Tenant) -> Result<Value, AppError> {
    let rows = state.db.list_invites_for_tenant(tenant.id).await?;
    let invites: Vec<Value> = rows
        .into_iter()
        .map(|(invite, invitees)| {
            let mut obj = serde_json::Map::new();
            obj.insert("kind".to_string(), json!(invite.kind));
            obj.insert("uses".to_string(), json!(invite.uses));
            obj.insert("invitees".to_string(), json!(invitees));
            obj.insert("revoked".to_string(), json!(invite.revoked_unix.is_some()));
            if invite.kind == "standard" {
                obj.insert("max_uses".to_string(), json!(invite.max_uses));
                obj.insert("expires_at".to_string(), json!(invite.expires_unix));
                obj.insert("share".to_string(), json!(invite.share));
            }
            Value::Object(obj)
        })
        .collect();
    Ok(json!({ "invites": invites }))
}

/// `host.invite.revoke {code}` (requirement 5 / AC7, requirement 11 /
/// AC10): a standard invite is simply marked revoked (existing invitees
/// keep their tenants and shares -- nothing else in storage changes). A
/// standing invite is rotated instead: the old code starts answering
/// `invite_invalid` and a freshly minted one takes its place, so the
/// tenant is never left without a standing invite of its own.
pub async fn revoke(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let code = arg_str(args, "code")?;
    let code_hash = crate::auth::hash_key(&code);
    let Some(invite) = state.db.revoke_invite(tenant.id, code_hash).await? else {
        return Err(AppError::invite_invalid());
    };
    if invite.kind == "standing" {
        let new_code = crate::auth::generate_url_secret();
        let new_code_hash = crate::auth::hash_key(&new_code);
        state
            .db
            .create_invite(
                tenant.id,
                "standing",
                new_code_hash,
                Some(new_code.clone()),
                Vec::new(),
                None,
                None,
                None,
            )
            .await?;
        return Ok(json!({
            "code": code,
            "revoked": true,
            "invite_url": invite_url(state, &new_code),
        }));
    }
    Ok(json!({ "code": code, "revoked": true }))
}

/// Requirement 10 (AC9): mints a tenant's standing invite -- `kind:
/// "standing"`, no expiry, no `max_uses`, `share: []`. Called once, right
/// after `control::signup` creates the tenant row (same "minted once,
/// right after the tenant row exists" convention `claim::issue_claim_token`
/// already uses there). [`claim_on_first_call`]'s own invite-created
/// tenant mints its own standing invite inline, inside the same atomic
/// transaction (`Db::claim_invite`) -- this function is only for the
/// plain-signup path, which has no such transaction to ride along with.
pub async fn mint_standing_invite(state: &AppState, tenant_id: i64) -> Result<(), AppError> {
    let code = crate::auth::generate_url_secret();
    let code_hash = crate::auth::hash_key(&code);
    state
        .db
        .create_invite(tenant_id, "standing", code_hash, Some(code), Vec::new(), None, None, None)
        .await?;
    Ok(())
}

/// The `<code>` segment of a `/i/{code}/mcp` request, read straight off
/// the original request's own URI -- same convention as `handler.rs`'s
/// own `url_path_secret`. `None` for every other path.
pub fn invite_path_code(parts: &http::request::Parts) -> Option<&str> {
    parts.uri.path().strip_prefix("/i/")?.strip_suffix("/mcp")
}

/// Requirement 2 (AC2), requirement 6: the whole implicit-signup-via-invite
/// side effect for one anonymous session's first `host.*` call on
/// `/i/{code}/mcp`. Binds `session_id` to the freshly created tenant
/// before returning (requirement 6: every later call on the same session
/// resolves through the ordinary session-binding step, never back here),
/// and returns the `onboarding` object `handler::call_tool` merges into
/// that first call's own response.
pub async fn claim_on_first_call(
    state: &AppState,
    code: &str,
    session_id: Option<&str>,
    funnel_origin: &str,
) -> Result<(Tenant, Value), AppError> {
    let code_hash = crate::auth::hash_key(code);
    let invitee_namespace = crate::auth::generate_namespace();
    let invitee_key_hash = crate::auth::hash_key(&crate::auth::generate_key());
    let invitee_url_secret = crate::auth::generate_url_secret();
    let invitee_url_secret_hash = crate::auth::hash_key(&invitee_url_secret);
    let invitee_standing_code = crate::auth::generate_url_secret();
    let invitee_standing_code_hash = crate::auth::hash_key(&invitee_standing_code);

    let claimed = state
        .db
        .claim_invite(
            code.to_string(),
            code_hash,
            invitee_namespace,
            invitee_key_hash,
            invitee_url_secret_hash,
            invitee_standing_code_hash,
            invitee_standing_code,
            funnel_origin.to_string(),
        )
        .await?;

    if let Some(session_id) = session_id {
        state
            .session_bindings
            .bind(session_id, claimed.tenant.id, crate::state::now_unix());
    }

    // PRD-mcphost-ownership-moment requirement 1 (AC1): `claim_invite`'s
    // own transaction mints the invitee's URL/standing invite but no claim
    // token (unlike `control::signup`) -- minted here, right after the
    // tenant row exists, same "issue right after creation" convention
    // `control::signup` already uses for its own claim_url.
    let claim_url = crate::claim::issue_claim_token(state, &claimed.tenant).await?;

    // Requirement 6: the invitee's own personal URL (PRD-mcphost-url-bound-tenants),
    // minted in the same call as the tenant itself.
    let onboarding = json!({
        "url": format!("{}/u/{}/mcp", state.public_url.trim_end_matches('/'), invitee_url_secret),
        "invited_by": claimed.inviter_namespace,
        "shared_tools": claimed.shared,
        "claim_url": claim_url,
    });
    Ok((claimed.tenant, onboarding))
}

/// Requirement 14 (AC13): "once per session" tracking for the
/// `_meta.invite_url` hint a shared-tool call's result carries on an
/// invitee's first such call in a session. In-memory only, the same
/// "never written to the database" posture as
/// [`crate::session_bind::SessionBindings`] -- a hint is a UX nudge, not
/// state anything else in this crate reads.
#[derive(Default, Clone)]
pub struct InviteHintTracker(Arc<Mutex<HashSet<String>>>);

impl InviteHintTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// `true` the first time this `session_id` is seen, `false` every
    /// time after.
    pub fn mark_first(&self, session_id: &str) -> bool {
        let Ok(mut seen) = self.0.lock() else { return false };
        seen.insert(session_id.to_string())
    }
}
