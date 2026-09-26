//! PRD-mcphost-end-user-audit-and-revoke: the tenant-facing control plane
//! for the end users its tools serve -- `host.enduser.list`/`get`/`audit`/
//! `revoke`/`unrevoke`/`purge`/`export` and `admin.enduser.stats`. Named
//! `enduserctl` (control plane) rather than widening `crate::enduser`
//! (identity resolution/scoping, PRD-mcphost-end-user-identity) -- the two
//! PRDs already read and write distinct tables (`end_users`/`tenant_audit`/
//! `vault_tokens` here, `tenant_state_kv`/`tenant_state_rows` there), so
//! this stays a sibling module rather than folding into that one.
//!
//! Requirement 1's batched upsert: every identified call records into
//! [`AppState::end_user_activity`] (an in-memory `(tenant, subject) ->
//! pending activity` map, `handler.rs` is the only writer via
//! [`EndUserActivityBuffer::record`]); [`flush_once`] drains it into the
//! `end_users` table every [`FLUSH_INTERVAL`] (10s), never per call
//! (non-functional: "the upsert batch adds no per-call latency"). A crash
//! loses at most one flush interval's `last_seen`/`calls_total` -- never a
//! revoke, which [`revoke`]/[`unrevoke`] write straight to the table
//! (technical considerations).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use crate::db::{EndUserRow, Tenant};
use crate::errors::AppError;
use crate::state::AppState;

/// requirement 1: "flushed every 10 s, not per call".
pub const FLUSH_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Default)]
struct PendingActivity {
    issuer: Option<String>,
    calls_delta: i64,
    last_seen: i64,
}

type ActivityKey = (i64, String);
type ActivityMap = HashMap<ActivityKey, PendingActivity>;
/// One drained entry: `(tenant_id, subject, issuer, last_seen, calls_delta)`.
type DrainedActivity = (i64, String, Option<String>, i64, i64);

/// See the module doc comment's "Requirement 1's batched upsert" section.
#[derive(Clone, Default)]
pub struct EndUserActivityBuffer(Arc<Mutex<ActivityMap>>);

impl EndUserActivityBuffer {
    /// Called once per identified call (`handler.rs`, right after its
    /// `calls` row is written) -- never touches the database itself, just
    /// the in-memory map [`flush_once`] later drains.
    pub fn record(&self, tenant_id: i64, subject: &str, issuer: Option<String>) {
        let now = crate::state::now_unix();
        if let Ok(mut map) = self.0.lock() {
            let entry = map.entry((tenant_id, subject.to_string())).or_default();
            entry.calls_delta += 1;
            entry.last_seen = now;
            if issuer.is_some() {
                entry.issuer = issuer;
            }
        }
    }

    fn drain(&self) -> Vec<DrainedActivity> {
        let Ok(mut map) = self.0.lock() else { return Vec::new() };
        map.drain()
            .map(|((tenant_id, subject), pending)| {
                (tenant_id, subject, pending.issuer, pending.last_seen, pending.calls_delta)
            })
            .collect()
    }
}

/// One flush cycle: drains [`AppState::end_user_activity`] and upserts
/// each pending `(tenant, subject)` into `end_users`. `pub` (not only
/// reachable through [`spawn_scheduler`]'s loop) so tests can flush
/// deterministically instead of waiting a real 10s -- same `tick_once`/
/// `spawn_scheduler` split `docs_index.rs` uses for its own 10s tick.
pub async fn flush_once(state: &AppState) -> Result<(), AppError> {
    for (tenant_id, subject, issuer, last_seen, calls_delta) in state.end_user_activity.drain() {
        state
            .db
            .upsert_end_user_activity(tenant_id, subject, issuer, last_seen, calls_delta)
            .await?;
    }
    Ok(())
}

/// requirement 1: started once at `serve` startup alongside every other
/// background tick (`main.rs`).
pub fn spawn_scheduler(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(FLUSH_INTERVAL).await;
            if let Err(e) = flush_once(&state).await {
                tracing::warn!(error = %e, "end-user activity flush failed");
            }
        }
    })
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

fn arg_i64_opt(args: &Value, name: &str) -> Option<i64> {
    args.get(name).and_then(Value::as_i64)
}

fn arg_bool_opt(args: &Value, name: &str) -> Option<bool> {
    args.get(name).and_then(Value::as_bool)
}

fn end_user_row_json(row: &EndUserRow) -> Value {
    json!({
        "subject": row.subject,
        "issuer": row.issuer,
        "first_seen": row.first_seen,
        "last_seen": row.last_seen,
        "calls_total": row.calls_total,
        "revoked_at": row.revoked_at,
        "revoked_by": row.revoked_by,
        "purged_at": row.purged_at,
    })
}

/// Opaque `list` cursor: base64 of `"<last_seen>:<subject>"`, the keyset
/// boundary the previous page stopped at.
fn encode_cursor(last_seen: i64, subject: &str) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(format!("{last_seen}:{subject}"))
}

fn decode_cursor(cursor: &str) -> Result<(i64, String), AppError> {
    use base64::Engine as _;
    let invalid = || AppError::InvalidArgs("cursor is not valid".to_string());
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| invalid())?;
    let raw = String::from_utf8(raw).map_err(|_| invalid())?;
    let (last_seen_raw, subject) = raw.split_once(':').ok_or_else(invalid)?;
    let last_seen = last_seen_raw.parse::<i64>().map_err(|_| invalid())?;
    Ok((last_seen, subject.to_string()))
}

const DEFAULT_LIST_LIMIT: i64 = 50;
const MAX_LIST_LIMIT: i64 = 1000;
/// `audit`'s per-source fetch cap (requirement 3): audit carries no scale
/// AC like `list`'s AC10, so it merges up to this many of each source in
/// Rust rather than a single cross-table keyset query.
const AUDIT_FETCH_CAP: i64 = 2000;

/// `host.enduser.list {since?, revoked?, limit?, cursor?}` (requirement 2 /
/// AC1/AC10).
pub async fn list(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let since = arg_i64_opt(args, "since");
    let revoked = arg_bool_opt(args, "revoked");
    let limit = arg_i64_opt(args, "limit").unwrap_or(DEFAULT_LIST_LIMIT).clamp(1, MAX_LIST_LIMIT);
    let cursor = match args.get("cursor") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(decode_cursor(s)?),
        Some(_) => return Err(AppError::InvalidArgs("cursor must be a string".to_string())),
    };

    // Fetch one extra row so a full page never has to guess whether
    // another page follows (a `rows.len() == limit` heuristic would treat
    // a page that exactly empties the table as "maybe more").
    let mut rows = state.db.list_end_users(tenant.id, since, revoked, limit + 1, cursor).await?;
    let has_more = rows.len() as i64 > limit;
    if has_more {
        rows.truncate(limit as usize);
    }
    let next_cursor = has_more.then(|| rows.last().map(|r| encode_cursor(r.last_seen, &r.subject))).flatten();
    let end_users: Vec<Value> = rows.iter().map(end_user_row_json).collect();
    Ok(json!({"end_users": end_users, "cursor": next_cursor}))
}

fn end_user_not_found(subject: &str) -> AppError {
    AppError::Structured {
        code: "end_user_not_found",
        message: format!("no end user '{subject}' for this tenant"),
        data: json!({"subject": subject}),
    }
}

/// `host.enduser.get {subject}` (requirement 2 / AC2): the roster row plus
/// `state_rows`/`vault_connections`/`runs_30d` counts read live from their
/// own tables (not cached on the roster row, unlike `calls_total`).
pub async fn get(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let subject = arg_str(args, "subject")?;
    let row = state
        .db
        .get_end_user(tenant.id, subject.clone())
        .await?
        .ok_or_else(|| end_user_not_found(&subject))?;

    let state_rows = state.db.count_state_rows_for_subject(tenant.id, subject.clone()).await?;
    let vault_connections = state.db.count_vault_connections_for_subject(tenant.id, subject.clone()).await?;
    let runs_30d = state.db.count_runs_30d_for_subject(tenant.id, subject).await?;

    let mut value = end_user_row_json(&row);
    if let Some(obj) = value.as_object_mut() {
        obj.insert("state_rows".to_string(), json!(state_rows));
        obj.insert("vault_connections".to_string(), json!(vault_connections));
        obj.insert("runs_30d".to_string(), json!(runs_30d));
    }
    Ok(value)
}

/// One `host.enduser.audit` entry: either a `calls` row (`kind: "call"`) or
/// a `tenant_audit` control-plane event (`kind: "event"`) -- `ts` is the
/// common sort key the two are merged on.
enum AuditEntry {
    Call { ts: i64, tool: String, outcome: String, run_id: Option<String> },
    Event { ts: i64, action: String, detail: Option<String> },
}

impl AuditEntry {
    fn ts(&self) -> i64 {
        match self {
            AuditEntry::Call { ts, .. } | AuditEntry::Event { ts, .. } => *ts,
        }
    }

    fn to_json(&self) -> Value {
        match self {
            // requirement 3: `impersonated` is always `false` here -- a
            // `calls` row (unlike a `host.state.*` op's own JSON result,
            // PRD-mcphost-end-user-identity requirement 5) carries no
            // impersonation concept of its own; every identified call's
            // `end_user_subject` on this ledger came from a verified OAuth
            // bearer or assertion, never an unauthenticated explicit
            // subject.
            AuditEntry::Call { ts, tool, outcome, run_id } => json!({
                "type": "call",
                "ts": ts,
                "tool": tool,
                "outcome": outcome,
                "run_id": run_id,
                "impersonated": false,
            }),
            AuditEntry::Event { ts, action, detail } => json!({
                "type": "event",
                "ts": ts,
                "action": action,
                "detail": detail,
            }),
        }
    }
}

/// `host.enduser.audit {subject, since?, limit?, cursor?}` (requirement 3 /
/// AC3/AC7): this subject's `calls` history merged with its `tenant_audit`
/// control-plane events, newest first. `cursor` is the previous page's
/// plain decimal offset into that merged, sorted list -- same convention
/// [`crate::agents::search`] uses for its own cursor, appropriate here for
/// the same reason: no scale AC to justify a keyset cursor.
pub async fn audit(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let subject = arg_str(args, "subject")?;
    let since = arg_i64_opt(args, "since");
    let limit: usize =
        arg_i64_opt(args, "limit").unwrap_or(DEFAULT_LIST_LIMIT).clamp(1, MAX_LIST_LIMIT) as usize;
    let offset: usize = match args.get("cursor") {
        None | Some(Value::Null) => 0,
        Some(Value::String(s)) => {
            s.parse().map_err(|_| AppError::InvalidArgs("cursor is not valid".to_string()))?
        }
        Some(_) => return Err(AppError::InvalidArgs("cursor must be a string".to_string())),
    };

    let calls = state.db.list_calls_for_subject(tenant.id, subject.clone(), since, AUDIT_FETCH_CAP).await?;
    let events = state.db.list_tenant_audit_for_subject(tenant.id, subject, since, AUDIT_FETCH_CAP).await?;

    let mut entries: Vec<AuditEntry> = Vec::with_capacity(calls.len() + events.len());
    entries.extend(calls.into_iter().map(|(ts, tool, outcome, run_id)| AuditEntry::Call {
        ts,
        tool,
        outcome,
        run_id,
    }));
    entries.extend(
        events.into_iter().map(|(ts, action, detail)| AuditEntry::Event { ts, action, detail }),
    );
    entries.sort_by_key(|a| std::cmp::Reverse(a.ts()));

    let total = entries.len();
    let page: Vec<Value> = entries.iter().skip(offset).take(limit).map(AuditEntry::to_json).collect();
    let next_offset = offset + page.len();
    let cursor = if next_offset < total { Some(next_offset.to_string()) } else { None };

    Ok(json!({"entries": page, "cursor": cursor}))
}

/// The only caller of `host.enduser.revoke`/`unrevoke`/`purge` is the
/// owning tenant itself (this control plane has no separate per-operator
/// actor identity the way `admin_audit`'s `actor_key_id` does) -- a fixed
/// string rather than a real actor id.
const REVOKED_BY_TENANT: &str = "tenant";

/// `host.enduser.revoke {subject, reason?}` (requirement 4 / AC4): sets
/// `revoked_at` (synchronously -- technical considerations: "a crash loses
/// at most 10s of last-seen updates, never a revoke") and disconnects
/// every live vault connection. The per-call refusal itself
/// (`end_user_revoked`) is enforced in `handler.rs`, not here.
pub async fn revoke(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let subject = arg_str(args, "subject")?;
    let reason = arg_str_opt(args, "reason");

    state.db.revoke_end_user(tenant.id, subject.clone(), REVOKED_BY_TENANT.to_string()).await?;
    state.db.revoke_vault_tokens_for_subject(tenant.id, subject.clone()).await?;
    state.db.record_tenant_audit(tenant.id, subject.clone(), "revoke".to_string(), reason).await?;

    Ok(json!({"subject": subject, "revoked": true}))
}

/// `host.enduser.unrevoke {subject}` (requirement 4 / AC7): reverses
/// `revoke` -- clears `revoked_at`/`revoked_by` so the subject's next
/// identified call is served again.
pub async fn unrevoke(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let subject = arg_str(args, "subject")?;
    let existed = state.db.unrevoke_end_user(tenant.id, subject.clone()).await?;
    if !existed {
        return Err(end_user_not_found(&subject));
    }
    state.db.record_tenant_audit(tenant.id, subject.clone(), "unrevoke".to_string(), None).await?;
    Ok(json!({"subject": subject, "revoked": false}))
}

/// `host.enduser.purge {subject}` (requirement 5 / AC5/AC6): requires
/// `revoked_at` already set (`revoke` first, same "deletion request
/// executable ... revoke+purge <= 2 calls" shape the PRD's own success
/// metric names) -- deletes the subject's scoped state and vault rows,
/// de-identifies its `calls` rows, and writes the audit tombstone.
pub async fn purge(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let subject = arg_str(args, "subject")?;
    let row = state
        .db
        .get_end_user(tenant.id, subject.clone())
        .await?
        .ok_or_else(|| end_user_not_found(&subject))?;
    if row.revoked_at.is_none() {
        return Err(AppError::Structured {
            code: "revoke_required",
            message: format!("end user '{subject}' must be revoked before it can be purged"),
            data: json!({"subject": subject}),
        });
    }

    let (state_rows, vault_tokens) = state.db.purge_end_user(tenant.id, subject.clone()).await?;
    Ok(json!({"subject": subject, "state_rows": state_rows, "vault_tokens": vault_tokens}))
}
