//! PRD-mcphost-oauth-client-policy: per-tenant OAuth client policy (`clients:
//! any|allowlist|approve`), session policy (access/refresh TTLs, max grant
//! age, re-consent interval), the tenant-readable auth audit log and
//! export, `host.oauth.revoke_all`, and the operator's global client block
//! list. `authz.rs` owns `/oauth/authorize`/`/oauth/token` themselves and
//! calls into this module at the points requirement 2 names (client
//! identification, token minting, refresh); this module owns the policy
//! row itself, the pending-client queue, the audit table, and every
//! `host.oauth.*`/`admin.oauth.*` tool this PRD adds.

use serde_json::{Value, json};

use crate::authz::ClientIdentity;
use crate::db::{OauthPolicyRow, Tenant};
use crate::errors::AppError;
use crate::state::{AppState, now_unix};

/// requirement 1 default, and the hosted AS PRD's own fixed constant
/// before this one -- "defaults reproduce the hosted AS PRD's behaviour
/// exactly".
pub const DEFAULT_ACCESS_TTL_S: i64 = 3600;
/// requirement 1 default: 30 days, the hosted AS PRD's own fixed refresh
/// TTL before this one.
pub const DEFAULT_REFRESH_TTL_S: i64 = 30 * 86_400;
pub const ACCESS_TTL_MIN: i64 = 300;
pub const ACCESS_TTL_MAX: i64 = 3600;
pub const REFRESH_TTL_MIN: i64 = 3600;
pub const REFRESH_TTL_MAX: i64 = 2_592_000;

/// requirement 6: 100 registrations per CIMD host per rolling day.
pub const CIMD_DAILY_REGISTRATION_LIMIT: i64 = 100;
const CIMD_DAILY_REGISTRATION_WINDOW_SECS: i64 = 86_400;

/// requirement 4: `host.oauth.audit_export`'s size cap.
pub const AUDIT_EXPORT_MAX_BYTES: usize = 50 * 1024 * 1024;

fn arg_str(args: &Value, name: &str) -> Result<String, AppError> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppError::InvalidArgs(format!("missing required argument '{name}'")))
}

fn arg_str_opt(args: &Value, name: &str) -> Option<String> {
    args.get(name).and_then(Value::as_str).map(str::to_string)
}

fn arg_i64(args: &Value, name: &str) -> Result<i64, AppError> {
    args.get(name)
        .and_then(Value::as_i64)
        .ok_or_else(|| AppError::InvalidArgs(format!("missing required argument '{name}'")))
}

fn arg_i64_opt(args: &Value, name: &str) -> Option<i64> {
    args.get(name).and_then(Value::as_i64)
}

// ---- policy -----------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientsMode {
    Any,
    Allowlist,
    Approve,
}

impl ClientsMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::Allowlist => "allowlist",
            Self::Approve => "approve",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "any" => Some(Self::Any),
            "allowlist" => Some(Self::Allowlist),
            "approve" => Some(Self::Approve),
            _ => None,
        }
    }
}

/// The effective policy for one tenant -- either its own `oauth_policies`
/// row, or [`OauthPolicy::default`] when it has never called
/// `host.oauth.policy_set` (requirement 1 / AC9).
#[derive(Debug, Clone)]
pub struct OauthPolicy {
    pub clients_mode: ClientsMode,
    pub allowlist: Vec<String>,
    pub access_ttl_s: i64,
    pub refresh_ttl_s: i64,
    pub max_grant_age_s: Option<i64>,
    pub reconsent_after_s: Option<i64>,
}

impl Default for OauthPolicy {
    fn default() -> Self {
        Self {
            clients_mode: ClientsMode::Any,
            allowlist: Vec::new(),
            access_ttl_s: DEFAULT_ACCESS_TTL_S,
            refresh_ttl_s: DEFAULT_REFRESH_TTL_S,
            max_grant_age_s: None,
            reconsent_after_s: None,
        }
    }
}

impl OauthPolicy {
    fn from_row(row: OauthPolicyRow) -> Self {
        Self {
            clients_mode: ClientsMode::parse(&row.clients_mode).unwrap_or(ClientsMode::Any),
            allowlist: serde_json::from_str(&row.allowlist_json).unwrap_or_default(),
            access_ttl_s: row.access_ttl_s,
            refresh_ttl_s: row.refresh_ttl_s,
            max_grant_age_s: row.max_grant_age_s,
            reconsent_after_s: row.reconsent_after_s,
        }
    }

    pub async fn load(state: &AppState, tenant_id: i64) -> Result<Self, AppError> {
        Ok(match state.db.find_oauth_policy(tenant_id).await? {
            Some(row) => Self::from_row(row),
            None => Self::default(),
        })
    }

    fn to_json(&self) -> Value {
        json!({
            "clients": self.clients_mode.as_str(),
            "allowlist": self.allowlist,
            "access_ttl_s": self.access_ttl_s,
            "refresh_ttl_s": self.refresh_ttl_s,
            "max_grant_age_s": self.max_grant_age_s,
            "reconsent_after_s": self.reconsent_after_s,
        })
    }
}

fn merge_i64_opt(args: &Value, name: &str, previous: Option<i64>) -> Result<Option<i64>, AppError> {
    match args.get(name) {
        None => Ok(previous),
        Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_i64()
            .map(Some)
            .ok_or_else(|| AppError::InvalidArgs(format!("{name} must be an integer"))),
    }
}

/// `host.oauth.policy_set` (requirement 1): every field is optional and,
/// when omitted, keeps this tenant's current value (or the default, for a
/// tenant with no policy row yet) -- so AC3/AC4's session-only tuning
/// doesn't have to re-specify `clients`/`allowlist` on every call.
pub async fn policy_set(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let previous = OauthPolicy::load(state, tenant.id).await?;

    let clients_mode = match args.get("clients") {
        None => previous.clients_mode,
        Some(Value::String(s)) => ClientsMode::parse(s)
            .ok_or_else(|| AppError::InvalidArgs("clients must be one of any|allowlist|approve".to_string()))?,
        Some(_) => return Err(AppError::InvalidArgs("clients must be a string".to_string())),
    };

    let allowlist = match args.get("allowlist") {
        None => previous.allowlist,
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| AppError::InvalidArgs("allowlist entries must be strings".to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err(AppError::InvalidArgs("allowlist must be an array of strings".to_string())),
    };

    let access_ttl_s = match args.get("access_ttl_s") {
        None => previous.access_ttl_s,
        Some(v) => v.as_i64().ok_or_else(|| AppError::InvalidArgs("access_ttl_s must be an integer".to_string()))?,
    };
    if !(ACCESS_TTL_MIN..=ACCESS_TTL_MAX).contains(&access_ttl_s) {
        return Err(AppError::InvalidArgs(format!(
            "access_ttl_s must be between {ACCESS_TTL_MIN} and {ACCESS_TTL_MAX}"
        )));
    }

    let refresh_ttl_s = match args.get("refresh_ttl_s") {
        None => previous.refresh_ttl_s,
        Some(v) => v.as_i64().ok_or_else(|| AppError::InvalidArgs("refresh_ttl_s must be an integer".to_string()))?,
    };
    if !(REFRESH_TTL_MIN..=REFRESH_TTL_MAX).contains(&refresh_ttl_s) {
        return Err(AppError::InvalidArgs(format!(
            "refresh_ttl_s must be between {REFRESH_TTL_MIN} and {REFRESH_TTL_MAX}"
        )));
    }

    let max_grant_age_s = merge_i64_opt(args, "max_grant_age_s", previous.max_grant_age_s)?;
    let reconsent_after_s = merge_i64_opt(args, "reconsent_after_s", previous.reconsent_after_s)?;

    let allowlist_json = serde_json::to_string(&allowlist)
        .map_err(|e| AppError::Internal(format!("could not encode allowlist: {e}")))?;
    state
        .db
        .upsert_oauth_policy(
            tenant.id,
            clients_mode.as_str().to_string(),
            allowlist_json,
            access_ttl_s,
            refresh_ttl_s,
            max_grant_age_s,
            reconsent_after_s,
        )
        .await?;
    let _ = record_audit(state, Some(tenant.id), "policy_change", None, None, None, None, None).await;

    Ok(OauthPolicy { clients_mode, allowlist, access_ttl_s, refresh_ttl_s, max_grant_age_s, reconsent_after_s }
        .to_json())
}

/// `host.oauth.policy`.
pub async fn policy_get(state: &AppState, tenant: &Tenant) -> Result<Value, AppError> {
    Ok(OauthPolicy::load(state, tenant.id).await?.to_json())
}

// ---- pending / approve / deny ------------------------------------------

/// `host.oauth.pending`.
pub async fn pending_list(state: &AppState, tenant: &Tenant) -> Result<Value, AppError> {
    let rows = state.db.list_oauth_pending_clients(tenant.id).await?;
    Ok(json!({
        "pending": rows.iter().map(|r| json!({
            "client_id": r.client_id,
            "client_name": r.client_name,
            "method": r.method,
            "created_at": r.created_unix,
        })).collect::<Vec<_>>(),
    }))
}

/// `host.oauth.client_approve {client_id}` (requirement 2 / AC2): moves a
/// pending client to a standing approval; its next authorize reaches
/// consent directly.
pub async fn client_approve(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let client_id = arg_str(args, "client_id")?;
    let removed = state.db.remove_oauth_pending_client(tenant.id, client_id.clone()).await?;
    if !removed {
        return Err(AppError::Structured {
            code: "client_not_found",
            message: format!("no pending client '{client_id}'"),
            data: json!({"client_id": client_id}),
        });
    }
    state.db.insert_oauth_approved_client(tenant.id, client_id.clone()).await?;
    let _ = record_audit(state, Some(tenant.id), "approved", Some(client_id.as_str()), None, None, None, None).await;
    Ok(json!({"approved": true, "client_id": client_id}))
}

/// `host.oauth.client_deny {client_id}` (requirement 2 / AC2): removes a
/// pending client and durably refuses it -- every later attempt reads
/// `client_not_allowed`, never re-queues as pending.
pub async fn client_deny(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let client_id = arg_str(args, "client_id")?;
    let removed = state.db.remove_oauth_pending_client(tenant.id, client_id.clone()).await?;
    if !removed {
        return Err(AppError::Structured {
            code: "client_not_found",
            message: format!("no pending client '{client_id}'"),
            data: json!({"client_id": client_id}),
        });
    }
    state.db.remove_oauth_approved_client(tenant.id, client_id.clone()).await?;
    state.db.insert_oauth_denied_client(tenant.id, client_id.clone()).await?;
    let _ = record_audit(state, Some(tenant.id), "denied", Some(client_id.as_str()), None, None, None, None).await;
    Ok(json!({"denied": true, "client_id": client_id}))
}

/// requirement 2 (AC1/AC2): `/oauth/authorize`'s policy decision for an
/// already-identified client, once the tenant is known (proven ownership
/// at the consent POST) -- `authz::post_authorize` is this function's only
/// caller.
pub enum ClientDecision {
    Allowed,
    Pending,
    Refused(&'static str),
}

pub async fn evaluate_client(
    state: &AppState,
    tenant: &Tenant,
    identity: &ClientIdentity,
) -> Result<ClientDecision, AppError> {
    let policy = OauthPolicy::load(state, tenant.id).await?;
    match policy.clients_mode {
        ClientsMode::Any => Ok(ClientDecision::Allowed),
        ClientsMode::Allowlist => {
            let host = if identity.method == "cimd" { cimd_host_of(&identity.client_id) } else { None };
            let allowed = policy
                .allowlist
                .iter()
                .any(|entry| entry == &identity.client_id || host.as_deref() == Some(entry.as_str()));
            if allowed {
                Ok(ClientDecision::Allowed)
            } else {
                Ok(ClientDecision::Refused("client_not_allowed"))
            }
        }
        ClientsMode::Approve => {
            if state.db.is_oauth_client_denied(tenant.id, identity.client_id.clone()).await? {
                return Ok(ClientDecision::Refused("client_not_allowed"));
            }
            if state.db.is_oauth_client_approved(tenant.id, identity.client_id.clone()).await? {
                return Ok(ClientDecision::Allowed);
            }
            state
                .db
                .upsert_oauth_pending_client(
                    tenant.id,
                    identity.client_id.clone(),
                    identity.client_name.clone(),
                    identity.method.to_string(),
                )
                .await?;
            Ok(ClientDecision::Pending)
        }
    }
}

// ---- revoke_all ---------------------------------------------------------

/// `host.oauth.revoke_all` (requirement 3 / AC5): every live grant, its
/// access tokens (jti denylist) and refresh tokens, plus the whole pending
/// queue -- one audit row per revoked grant.
pub async fn revoke_all(state: &AppState, tenant: &Tenant) -> Result<Value, AppError> {
    let now = now_unix();
    let pending_before = state.db.list_oauth_pending_clients(tenant.id).await?.len();
    let revoked = state.db.revoke_all_oauth_grants_for_tenant(tenant.id, now).await?;
    state.db.clear_oauth_pending_clients_for_tenant(tenant.id).await?;
    for (_id, client_id) in &revoked {
        let _ = record_audit(state, Some(tenant.id), "revoke", Some(client_id.as_str()), None, None, None, None).await;
    }
    Ok(json!({"revoked_grants": revoked.len(), "revoked_pending": pending_before}))
}

// ---- audit / audit_export ------------------------------------------------

fn audit_row_json(row: &crate::db::OauthAuditRow) -> Value {
    json!({
        "id": row.id,
        "ts": row.ts,
        "event": row.event,
        "client_id": row.client_id,
        "method": row.method,
        "end_user_subject": row.end_user_subject,
        "reason": row.reason,
        "ip_hash": row.ip_hash,
        "scopes": row.scopes,
    })
}

const AUDIT_DEFAULT_LIMIT: i64 = 50;
const AUDIT_MAX_LIMIT: i64 = 500;

/// `host.oauth.audit {since?, until?, event?, limit?, cursor?}`
/// (requirement 4 / AC6): a plain-offset cursor, same convention
/// `enduserctl::audit` already uses.
pub async fn audit(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let since = arg_i64_opt(args, "since");
    let until = arg_i64_opt(args, "until");
    let event = arg_str_opt(args, "event");
    let limit = arg_i64_opt(args, "limit").unwrap_or(AUDIT_DEFAULT_LIMIT).clamp(1, AUDIT_MAX_LIMIT);
    let offset = match args.get("cursor") {
        None | Some(Value::Null) => 0,
        Some(Value::String(s)) => {
            s.parse::<i64>().map_err(|_| AppError::InvalidArgs("cursor is not valid".to_string()))?
        }
        Some(_) => return Err(AppError::InvalidArgs("cursor must be a string".to_string())),
    };

    let mut rows = state.db.list_oauth_audit(tenant.id, since, until, event, limit + 1, offset).await?;
    let has_more = rows.len() as i64 > limit;
    rows.truncate(limit as usize);
    let events: Vec<Value> = rows.iter().map(audit_row_json).collect();
    let next_cursor = has_more.then(|| (offset + limit).to_string());
    Ok(json!({"events": events, "cursor": next_cursor}))
}

/// `host.oauth.audit_export {since, until}` (requirement 4 / AC6): every
/// matching row as one JSON-line string per entry -- identical field shape
/// [`audit`] pages, so a page and the matching export slice compare equal
/// once each line is parsed back to JSON. `export_too_large` names a
/// narrower (halved) window to retry with instead of leaving the caller to
/// guess one.
pub async fn audit_export(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let since = arg_i64(args, "since")?;
    let until = arg_i64(args, "until")?;
    if until < since {
        return Err(AppError::InvalidArgs("until must not be before since".to_string()));
    }
    let rows = state.db.list_oauth_audit(tenant.id, Some(since), Some(until), None, 10_000_000, 0).await?;
    let lines: Vec<String> = rows.iter().map(|r| audit_row_json(r).to_string()).collect();
    let total_bytes: usize = lines.iter().map(|l| l.len() + 1).sum();
    if total_bytes > AUDIT_EXPORT_MAX_BYTES {
        let suggested_until = since + ((until - since) / 2).max(1);
        return Err(AppError::Structured {
            code: "export_too_large",
            message: format!(
                "export exceeds {AUDIT_EXPORT_MAX_BYTES} bytes for this window; try a narrower one"
            ),
            data: json!({"since": since, "until": suggested_until}),
        });
    }
    Ok(json!({"lines": lines}))
}

// ---- operator block list / stats (admin.oauth.*) -------------------------

fn block_target(args: &Value) -> Result<(&'static str, String), AppError> {
    match (args.get("client_id").and_then(Value::as_str), args.get("cimd_host").and_then(Value::as_str)) {
        (Some(c), None) => Ok(("client_id", c.to_string())),
        (None, Some(h)) => Ok(("cimd_host", h.to_string())),
        _ => Err(AppError::InvalidArgs("exactly one of client_id or cimd_host is required".to_string())),
    }
}

/// `admin.oauth.client_block {client_id | cimd_host, reason}` (requirement
/// 5 / AC7).
pub async fn admin_client_block(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let (kind, value) = block_target(args)?;
    let reason = arg_str_opt(args, "reason");
    state.db.upsert_oauth_client_block(kind.to_string(), value.clone(), reason.clone()).await?;
    Ok(json!({"blocked": true, "kind": kind, "value": value, "reason": reason}))
}

/// `admin.oauth.client_unblock {client_id | cimd_host}`.
pub async fn admin_client_unblock(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let (kind, value) = block_target(args)?;
    let removed = state.db.remove_oauth_client_block(kind.to_string(), value.clone()).await?;
    Ok(json!({"unblocked": removed, "kind": kind, "value": value}))
}

/// `admin.oauth.blocked`.
pub async fn admin_blocked_list(state: &AppState) -> Result<Value, AppError> {
    let rows = state.db.list_oauth_client_blocks().await?;
    Ok(json!({
        "blocked": rows.iter().map(|r| json!({
            "kind": r.kind,
            "value": r.value,
            "reason": r.reason,
            "created_at": r.created_unix,
        })).collect::<Vec<_>>(),
    }))
}

/// `admin.oauth.stats` (requirement 5 / AC7-AC8): total blocks on the
/// list, plus how many attempts each of the two block-triggered refusal
/// reasons has cost across every tenant.
pub async fn admin_stats(state: &AppState) -> Result<Value, AppError> {
    let blocks = state.db.count_oauth_client_blocks().await?;
    let client_blocked = state.db.count_oauth_audit_by_reason("client_blocked".to_string()).await?;
    let cimd_rate_limited = state.db.count_oauth_audit_by_reason("cimd_registration_rate_limited".to_string()).await?;
    Ok(json!({
        "blocks": blocks,
        "refused_client_blocked": client_blocked,
        "refused_cimd_rate_limited": cimd_rate_limited,
    }))
}

// ---- shared helpers used by authz.rs -------------------------------------

/// requirement 5: the CIMD host is the hostname of the `client_id` URL --
/// `None` for a DCR `client_id` (never a URL at all), which every caller
/// treats as "no host to match a `cimd_host` block/allowlist entry
/// against".
pub fn cimd_host_of(client_id: &str) -> Option<String> {
    reqwest::Url::parse(client_id).ok().and_then(|u| u.host_str().map(str::to_string))
}

/// requirement 5 (AC7): checked ahead of any tenant policy -- `true` iff
/// this exact `client_id`, or (for a CIMD client) its host, is on the
/// operator's block list. Takes a bare `client_id`/`method` rather than a
/// full [`ClientIdentity`] so `/oauth/token`'s refresh path (which only
/// ever re-reads an already-completed grant, never re-identifies the
/// client through [`crate::authz::identify_client`]) can call it too.
pub async fn is_client_id_or_method_blocked(state: &AppState, client_id: &str, method: &str) -> Result<bool, AppError> {
    let host = if method == "cimd" { cimd_host_of(client_id) } else { None };
    state.db.oauth_client_is_blocked(client_id.to_string(), host).await
}

pub async fn is_client_blocked(state: &AppState, identity: &ClientIdentity) -> Result<bool, AppError> {
    is_client_id_or_method_blocked(state, &identity.client_id, identity.method).await
}

/// requirement 6 (AC8): `true` iff this CIMD client's host is still under
/// its 100-per-day cap (and admits it); always `true` for a non-CIMD
/// client (a DCR registration has its own, separate per-IP limit in
/// `authz::post_register`).
pub async fn admit_cimd_registration(state: &AppState, identity: &ClientIdentity) -> Result<bool, AppError> {
    if identity.method != "cimd" {
        return Ok(true);
    }
    let Some(host) = cimd_host_of(&identity.client_id) else {
        return Ok(true);
    };
    let since = now_unix() - CIMD_DAILY_REGISTRATION_WINDOW_SECS;
    state.db.try_admit_cimd_registration(host, since, CIMD_DAILY_REGISTRATION_LIMIT).await
}

/// Technical considerations: `ip_hash` is a salted hash, never the raw
/// address.
pub fn ip_hash(state: &AppState, ip: &str) -> String {
    state.secrets.salted_hash(ip)
}

/// requirement 4: appends one `oauth_audit` row. Best-effort at every call
/// site (`let _ = record_audit(...).await;`), same posture every other
/// grant-bookkeeping side effect in `authz.rs` already takes -- a failure
/// to journal an event must never fail the request it's describing.
#[allow(clippy::too_many_arguments)]
pub async fn record_audit(
    state: &AppState,
    tenant_id: Option<i64>,
    event: &str,
    client_id: Option<&str>,
    method: Option<&str>,
    end_user_subject: Option<&str>,
    reason: Option<&str>,
    ip: Option<&str>,
) -> Result<(), AppError> {
    let ip_hash = ip.map(|raw| ip_hash(state, raw));
    state
        .db
        .insert_oauth_audit(
            tenant_id,
            event.to_string(),
            client_id.map(str::to_string),
            method.map(str::to_string),
            end_user_subject.map(str::to_string),
            reason.map(str::to_string),
            ip_hash,
            Some("mcp".to_string()),
        )
        .await?;
    Ok(())
}
