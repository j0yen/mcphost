//! Business logic for `host.agent.contact_request` / `contacts` /
//! `contact_accept` / `contact_deny` / `mute` / `unmute` / `contacts_import`
//! (PRD-mcphost-agent-consent), plus the one shared decision this PRD's own
//! technical considerations ask for: "the inbox PRD's resolution function
//! gains one call into `consent::check(sender, recipient) -> Allow |
//! Refuse(code)`; keep it a single function so mesh-ops can later log the
//! decision". [`check`] (and the smaller `&Connection`-level helpers it's
//! built from) is that function -- called from both `Db::resolve_message_
//! recipient` (`host.msg.send`) and `Db::msg_reply`'s per-participant loop,
//! so the two can't drift the way they briefly did before this PRD (0021's
//! `resolve_message_recipient` and `msg_reply` each carried their own copy
//! of the blocked/policy check).
//!
//! Same module split as `messaging.rs`/`agents.rs`: the tool-facing half
//! below (`contact_request`, `contacts`, `contact_accept`, `contact_deny`,
//! `mute`, `unmute`, `contacts_import`) is pure `AppState` + arguments in,
//! `serde_json::Value` (or [`AppError`]) out, delegating storage to new
//! `Db` methods (`db.rs`'s "---- consent ----" section). The lower half
//! (`check`/`is_blocked`/`has_accepted_contact`/`classify_existing_request`)
//! is the one exception to "db.rs is the only place that touches
//! rusqlite": it's written against a plain `&rusqlite::Connection` so
//! `Db::resolve_message_recipient`/`Db::msg_reply`/`Db::contact_request` can
//! all call it from *inside* their own already-open transactions, the same
//! way they already call `Db::query_agent_profile`.

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

use crate::db::Tenant;
use crate::errors::AppError;
use crate::state::AppState;

/// Requirement 11 (P2, lazily enforced -- no spawned housekeeping task,
/// see the migration comment and `classify_existing_request` below): a
/// `pending` request older than this is treated as `expired` the next time
/// anything reads it.
pub const REQUEST_EXPIRY_MS: i64 = 30 * 24 * 3_600_000;
/// Requirement 3 (AC4): how long a `denied` request blocks a fresh one.
pub const DENY_COOLDOWN_MS: i64 = 7 * 24 * 3_600_000;

// ---- shared decision (called from db.rs) --------------------------------

/// A resolved `host.msg.send`/`reply` consent decision -- deliberately the
/// same two-armed shape as `db.rs`'s own `RecipientResolution`, which is
/// what both call sites turn this straight into.
pub enum SendCheck {
    Allow,
    Refuse(&'static str),
}

/// `blocks` is unconditional and symmetric-agnostic: `blocker_id` blocked
/// `blockee_id` iff a `(blocker_id, blockee_id)` row exists. Shared by
/// [`check`] and `Db::contact_request` (AC7: a blocked caller's
/// `contact_request` must read identically to one against a nonexistent
/// address -- see [`check`]'s own doc comment for why that's
/// `agent_not_found`, not `contact_refused`).
pub fn is_blocked(conn: &Connection, blocker_id: i64, blockee_id: i64) -> Result<bool, AppError> {
    Ok(conn
        .prepare("SELECT 1 FROM blocks WHERE tenant_id = ?1 AND blocked_tenant_id = ?2")?
        .exists(params![blocker_id, blockee_id])?)
}

/// Migration 0023's `contacts` table stores an accepted pair as two rows
/// (one per direction -- see that migration's own doc comment), so this is
/// a plain single-row lookup from either side.
pub fn has_accepted_contact(
    conn: &Connection,
    tenant_id: i64,
    contact_tenant_id: i64,
) -> Result<bool, AppError> {
    Ok(conn
        .prepare("SELECT 1 FROM contacts WHERE tenant_id = ?1 AND contact_tenant_id = ?2")?
        .exists(params![tenant_id, contact_tenant_id])?)
}

/// One pending `contact_requests` row still within [`REQUEST_EXPIRY_MS`] --
/// what [`ExistingRequestState::Pending`] carries back to `Db::contact_request`
/// so a repeat call while pending can return the same request rather than
/// erroring (requirement 2: "creates or returns the pending request").
pub struct ExistingPendingRequest {
    pub request_id: String,
    pub note: Option<String>,
    pub created_at: String,
}

/// What [`classify_existing_request`] finds for one `(from, to)` pair.
pub enum ExistingRequestState {
    /// No `contact_requests` row at all.
    None,
    /// `status = 'pending'` and not yet past [`REQUEST_EXPIRY_MS`].
    Pending(ExistingPendingRequest),
    /// `status = 'denied'` and still within [`DENY_COOLDOWN_MS`] of
    /// `decided_at` (AC4).
    Cooldown,
    /// `status = 'accepted'`/`'expired'`, or a `pending` row just now
    /// lazily expired (requirement 11) or a `denied` row past cooldown --
    /// every one of these reads as "no active request" to a caller: a
    /// fresh `contact_request` may create a new `pending` row, and a plain
    /// send/reply refuses `contact_refused` rather than `contact_pending`.
    Stale,
}

/// Requirement 3 (AC2, AC4) / requirement 11 (P2, lazy expiry): the ONE
/// place that reads `contact_requests` for "is there an active request
/// between these two tenants right now" -- shared by [`check`] (which only
/// needs the pending-vs-not distinction) and `Db::contact_request` (which
/// also needs the pending row's own id/note/created_at to return it
/// unchanged on a repeat call). A `pending` row older than
/// [`REQUEST_EXPIRY_MS`] is opportunistically flipped to `'expired'` right
/// here (requirement 11: "housekeeping tick" -- there is no spawned task
/// for this, see migration 0023's own comment; every read of a stale
/// pending row corrects it instead).
pub fn classify_existing_request(
    conn: &Connection,
    from_tenant_id: i64,
    to_tenant_id: i64,
    now_ms: i64,
) -> Result<ExistingRequestState, AppError> {
    type Row = (String, String, Option<String>, String, i64, Option<i64>);
    let existing: Option<Row> = conn
        .query_row(
            "SELECT id, status, note, created_at, created_unix_ms, decided_unix_ms \
             FROM contact_requests WHERE from_tenant_id = ?1 AND to_tenant_id = ?2",
            params![from_tenant_id, to_tenant_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .optional()?;
    let Some((id, status, note, created_at, created_unix_ms, decided_unix_ms)) = existing else {
        return Ok(ExistingRequestState::None);
    };
    match status.as_str() {
        "pending" => {
            if now_ms - created_unix_ms > REQUEST_EXPIRY_MS {
                conn.execute(
                    "UPDATE contact_requests SET status = 'expired' WHERE id = ?1",
                    params![id],
                )?;
                Ok(ExistingRequestState::Stale)
            } else {
                Ok(ExistingRequestState::Pending(ExistingPendingRequest {
                    request_id: id,
                    note,
                    created_at,
                }))
            }
        }
        "denied" => {
            let decided_unix_ms = decided_unix_ms.unwrap_or(0);
            if now_ms - decided_unix_ms < DENY_COOLDOWN_MS {
                Ok(ExistingRequestState::Cooldown)
            } else {
                Ok(ExistingRequestState::Stale)
            }
        }
        // "accepted"/"expired": no active request either way.
        _ => Ok(ExistingRequestState::Stale),
    }
}

/// The shared decision requirement 4's technical considerations ask for:
/// may `sender_id` reach `recipient_id`, whose `contact_policy` is
/// `policy`? Called from `Db::resolve_message_recipient` (`host.msg.send`)
/// and `Db::msg_reply`'s per-participant loop -- both used to carry their
/// own copy of the blocked/policy check (0021's `resolve_message_recipient`
/// doc comment still describes the pre-consent version), so a future
/// change to either one no longer risks drifting from the other.
///
/// `urgent` sends take exactly this same decision (requirement 6: "allowed
/// only to accepted contacts or open recipients ... refuses urgent=true
/// sends the same as any other send") -- there is deliberately no `urgent`
/// parameter here; the urgent-specific `urgent_per_day` quota is a
/// *separate*, additive check `Db::msg_send` applies only once this
/// already says [`SendCheck::Allow`].
pub fn check(
    conn: &Connection,
    sender_id: i64,
    recipient_id: i64,
    policy: &str,
    now_ms: i64,
) -> Result<SendCheck, AppError> {
    // Blocked-by-recipient always reads as "doesn't exist" (0021's own
    // precedent, msg_ac05_block.rs AC5) -- checked before policy so a
    // blocked caller can never distinguish "blocked" from "unknown" by
    // comparing a `contacts`-policy refusal to a `closed`-policy one.
    if is_blocked(conn, recipient_id, sender_id)? {
        return Ok(SendCheck::Refuse("agent_not_found"));
    }
    match policy {
        "closed" => Ok(SendCheck::Refuse("contact_refused")),
        "contacts" => {
            if has_accepted_contact(conn, sender_id, recipient_id)? {
                return Ok(SendCheck::Allow);
            }
            match classify_existing_request(conn, sender_id, recipient_id, now_ms)? {
                ExistingRequestState::Pending(_) | ExistingRequestState::Cooldown => {
                    Ok(SendCheck::Refuse("contact_pending"))
                }
                ExistingRequestState::None | ExistingRequestState::Stale => {
                    Ok(SendCheck::Refuse("contact_refused"))
                }
            }
        }
        // "open" (and any value `query_agent_profile` hasn't already
        // defaulted to "open" -- there shouldn't be one, `agents::
        // validate_contact_policy` only ever writes these three).
        _ => Ok(SendCheck::Allow),
    }
}

// ---- host.agent.contact_request / contacts / contact_accept / ----------
// ---- contact_deny / mute / unmute / contacts_import ---------------------

const MAX_NOTE_BYTES: usize = 512;
const MAX_IMPORT_ADDRESSES: usize = 50;

fn arg_str(args: &Value, name: &str) -> Result<String, AppError> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppError::InvalidArgs(format!("missing required argument '{name}'")))
}

fn arg_str_opt(args: &Value, name: &str) -> Option<String> {
    args.get(name).and_then(Value::as_str).map(str::to_string)
}

fn validate_note(raw: &str) -> Result<String, AppError> {
    if raw.len() > MAX_NOTE_BYTES {
        return Err(AppError::InvalidArgs(format!(
            "note: {} bytes, over the {MAX_NOTE_BYTES} byte limit",
            raw.len()
        )));
    }
    Ok(raw.to_string())
}

fn arg_note(args: &Value) -> Result<Option<String>, AppError> {
    match args.get("note") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(validate_note(s)?)),
        Some(_) => Err(AppError::InvalidArgs("note: must be a string".to_string())),
    }
}

fn plan_for<'a>(state: &'a AppState, tenant: &Tenant) -> Result<&'a crate::plans::Plan, AppError> {
    state
        .plans
        .get(&tenant.plan)
        .ok_or_else(|| AppError::Internal(format!("tenant plan '{}' not in catalog", tenant.plan)))
}

/// `host.agent.contact_request(address, note?)` (requirement 2 / AC1, AC2,
/// AC4, AC7): `Db::contact_request` returns the pending row's own fields on
/// success; every refusal (`not_needed`, `contact_refused`,
/// `contact_pending`, quota-exceeded, or a blocked/nonexistent address's
/// `agent_not_found`) comes back as an `Err(AppError)` directly from that
/// method, so this is just argument validation plus a pass-through.
pub async fn contact_request(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let address = arg_str(args, "address")?;
    let note = arg_note(args)?;
    let plan = plan_for(state, tenant)?;
    let pending = state
        .db
        .contact_request(tenant.clone(), address.clone(), note, plan.contact_requests_per_day)
        .await?;
    Ok(json!({
        "status": "pending",
        "request_id": pending.request_id,
        "address": address,
        "note": pending.note,
        "created_at": pending.created_at,
    }))
}

fn contact_request_json(row: &crate::db::ContactRequestRow) -> Value {
    json!({
        "request_id": row.request_id,
        "address": row.address,
        "note": row.note,
        "status": row.status,
        "created_at": row.created_at,
        "decided_at": row.decided_at,
    })
}

/// `host.agent.contacts(status?)` (requirement 3 / AC2, AC3): the caller's
/// accepted contacts plus every pending/decided request in either
/// direction, optionally filtered to one `status`. `contacts` itself
/// (accepted pairs) has no `status` of its own -- the filter only narrows
/// `incoming`/`outgoing`.
pub async fn contacts(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let status = match arg_str_opt(args, "status") {
        None => None,
        Some(s) => Some(validate_status(&s)?),
    };
    let view = state.db.agent_contacts(tenant.id, status).await?;
    Ok(json!({
        "contacts": view.contacts.iter().map(|c| json!({
            "address": c.address,
            "accepted_at": c.accepted_at,
        })).collect::<Vec<_>>(),
        "incoming": view.incoming.iter().map(contact_request_json).collect::<Vec<_>>(),
        "outgoing": view.outgoing.iter().map(contact_request_json).collect::<Vec<_>>(),
    }))
}

fn validate_status(raw: &str) -> Result<String, AppError> {
    match raw {
        "pending" | "accepted" | "denied" | "expired" => Ok(raw.to_string()),
        other => Err(AppError::InvalidArgs(format!(
            "status: must be one of pending, accepted, denied, expired; got '{other}'"
        ))),
    }
}

/// `host.agent.contact_accept(request_id)` (requirement 3 / AC3).
pub async fn contact_accept(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let request_id = arg_str(args, "request_id")?;
    let decision = state.db.contact_accept(tenant.id, request_id).await?;
    Ok(json!({
        "request_id": decision.request_id,
        "status": "accepted",
        "address": decision.other_address,
        "accepted_at": decision.decided_at,
    }))
}

/// `host.agent.contact_deny(request_id)` (requirement 3 / AC4).
pub async fn contact_deny(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let request_id = arg_str(args, "request_id")?;
    let decision = state.db.contact_deny(tenant.id, request_id).await?;
    Ok(json!({
        "request_id": decision.request_id,
        "status": "denied",
        "address": decision.other_address,
    }))
}

/// `host.agent.mute(address)` (requirement 5).
pub async fn mute(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let address = arg_str(args, "address")?;
    let target_id = state
        .db
        .resolve_agent_address(address.clone())
        .await?
        .ok_or_else(AppError::agent_not_found)?;
    state.db.msg_mute(tenant.id, target_id).await?;
    Ok(json!({"address": address, "muted": true}))
}

/// `host.agent.unmute(address)` (requirement 5).
pub async fn unmute(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let address = arg_str(args, "address")?;
    let target_id = state
        .db
        .resolve_agent_address(address.clone())
        .await?
        .ok_or_else(AppError::agent_not_found)?;
    let unmuted = state.db.msg_unmute(tenant.id, target_id).await?;
    Ok(json!({"address": address, "unmuted": unmuted}))
}

/// `host.agent.contacts_import(addresses)` (requirement 10, P1): up to
/// [`MAX_IMPORT_ADDRESSES`] addresses, each resolved the same way a single
/// `contact_request` would be -- but per-address failures (already
/// connected, already pending, blocked, quota, ...) are reported in the
/// result list rather than failing the whole call, so this calls
/// `Db::contact_request` directly per address and reads each `Result` into
/// a status string (`AppError::code()`) instead of routing through
/// [`contact_request`] above, which would turn the first failure into a
/// whole-call `Err`.
pub async fn contacts_import(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let addresses: Vec<String> = match args.get("addresses") {
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| AppError::InvalidArgs("addresses: every entry must be a string".to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err(AppError::InvalidArgs("addresses: must be an array of addresses".to_string())),
        None => return Err(AppError::InvalidArgs("missing required argument 'addresses'".to_string())),
    };
    if addresses.is_empty() {
        return Err(AppError::InvalidArgs("addresses: must name at least one address".to_string()));
    }
    if addresses.len() > MAX_IMPORT_ADDRESSES {
        return Err(AppError::InvalidArgs(format!(
            "addresses: {} addresses, over the {MAX_IMPORT_ADDRESSES} per-call limit",
            addresses.len()
        )));
    }
    let plan = plan_for(state, tenant)?;
    let mut results = Vec::with_capacity(addresses.len());
    for address in addresses {
        let entry = match state
            .db
            .contact_request(tenant.clone(), address.clone(), None, plan.contact_requests_per_day)
            .await
        {
            Ok(pending) => json!({
                "address": address,
                "status": "pending",
                "request_id": pending.request_id,
            }),
            Err(e) => json!({"address": address, "status": e.code()}),
        };
        results.push(entry);
    }
    Ok(json!({"results": results}))
}
