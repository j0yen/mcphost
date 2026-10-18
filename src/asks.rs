//! PRD-mcphost-ask-user-decision: `host.ask.create` hands the user a signed
//! question link, `host.ask.wait` blocks the agent until the user answers,
//! the ask expires, or the wait ceiling ([`WAIT_MAX_S`]) says call again.
//!
//! Storage is migration 0083 (`asks`, `ask_events`). The state machine is
//! `pending -> answered | expired`, once: the answering `UPDATE` is guarded
//! by `status = 'pending'`, so the first answer wins and a later one learns
//! the recorded choice instead (see [`record_answer`]).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use tokio::sync::Notify;

use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::body::Bytes;
use std::net::SocketAddr;

use crate::auth::{constant_time_eq, hash_key};
use crate::claim::{html_escape, html_response};
use crate::db::Tenant;
use crate::errors::AppError;
use crate::state::{AppState, new_ulid, now_unix};

/// The wait ceiling and default live in [`crate::longpoll`]; re-exported so
/// the ask verbs and their callers keep one path.
pub use crate::longpoll::{WAIT_DEFAULT_S, WAIT_MAX_S};

pub const QUESTION_MAX_CHARS: usize = 500;
pub const OPTIONS_MIN: usize = 2;
pub const OPTIONS_MAX: usize = 6;
pub const OPTION_MAX_CHARS: usize = 40;
pub const CONTEXT_MAX_BYTES: usize = 2048;
pub const TTL_DEFAULT_S: i64 = 3600;
pub const TTL_MAX_S: i64 = 7 * 24 * 3600;

/// How often a blocked wait re-reads its row even with no wake-up, so an
/// answer recorded by another process is still seen.
const REREAD_EVERY: Duration = Duration::from_secs(1);

/// Verbs that read an ask and so are meaningless without a tenant: a
/// key-less call to one is `tenant_key_missing`, never an implicit signup
/// (the 0080 block refuses a second signup; these must not make a first).
pub const NEVER_SIGNS_UP: &[&str] = &["host.ask.wait", "host.ask.get", "host.ask.list"];

/// The path a signed ask link points at.
pub fn ask_path(id: &str) -> String {
    format!("/a/{id}")
}

/// The HMAC of an ask link, keyed by the owning tenant's `key_hash` exactly
/// as `/charts/` links are.
pub fn sign(key_hash: &str, id: &str, expires_unix: i64) -> String {
    crate::chart::sign_path(key_hash, &ask_path(id), expires_unix)
}

pub fn signed_ask_url(public_url: &str, key_hash: &str, id: &str, expires_unix: i64) -> String {
    format!(
        "{}{}?exp={expires_unix}&sig={}",
        public_url.trim_end_matches('/'),
        ask_path(id),
        sign(key_hash, id, expires_unix)
    )
}

// ---- wake-ups ---------------------------------------------------------------

/// One [`Notify`] per ask with a blocked waiter, woken by the answering
/// handler. Process-wide (ask ids are unique ULIDs) rather than an
/// `AppState` field, so no `AppState` literal in the crate or its tests
/// has to learn about it; the 1 s re-read in [`wait`] covers the case of
/// another process answering.
fn notifiers() -> &'static Mutex<HashMap<String, Arc<Notify>>> {
    static NOTIFIERS: OnceLock<Mutex<HashMap<String, Arc<Notify>>>> = OnceLock::new();
    NOTIFIERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn notifier(id: &str) -> Arc<Notify> {
    let mut map = notifiers().lock().unwrap_or_else(|e| e.into_inner());
    map.entry(id.to_string()).or_insert_with(|| Arc::new(Notify::new())).clone()
}

/// Wakes every waiter blocked on `id` and forgets the entry.
pub fn wake(id: &str) {
    let removed = notifiers().lock().unwrap_or_else(|e| e.into_inner()).remove(id);
    if let Some(n) = removed {
        n.notify_waiters();
    }
}

fn release_notifier(id: &str, mine: Arc<Notify>) {
    let mut map = notifiers().lock().unwrap_or_else(|e| e.into_inner());
    // The map's copy and this waiter's: nobody else is blocked on it.
    if Arc::strong_count(&mine) <= 2 {
        map.remove(id);
    }
}

// ---- rows ---------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct AskRow {
    pub id: String,
    pub tenant_id: i64,
    pub question: String,
    pub options: Vec<String>,
    pub context: Option<String>,
    pub status: String,
    pub choice: Option<String>,
    pub created_unix: i64,
    pub expires_unix: i64,
    pub answered_unix: Option<i64>,
    pub waited_total_ms: i64,
    /// `link` or `client` (PRD-mcphost-elicitation-with-ask-fallback R5).
    pub channel: String,
}

const ROW_COLUMNS: &str = "id, tenant_id, question, options_json, context, status, choice, \
                           created_unix, expires_unix, answered_unix, waited_total_ms, channel";

fn row_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<AskRow> {
    let options_json: String = r.get(3)?;
    Ok(AskRow {
        id: r.get(0)?,
        tenant_id: r.get(1)?,
        question: r.get(2)?,
        options: serde_json::from_str(&options_json).unwrap_or_default(),
        context: r.get(4)?,
        status: r.get(5)?,
        choice: r.get(6)?,
        created_unix: r.get(7)?,
        expires_unix: r.get(8)?,
        answered_unix: r.get(9)?,
        waited_total_ms: r.get(10)?,
        channel: r.get::<_, Option<String>>(11)?.unwrap_or_else(|| "link".to_string()),
    })
}

/// Marks every pending ask whose `expires_unix` has passed as `expired`,
/// with its one `expired` audit event stamped at the moment it lapsed.
/// Idempotent: the guarded `UPDATE` only fires once per ask.
pub(crate) fn expire_due(conn: &Connection, now: i64) -> Result<(), AppError> {
    let mut stmt = conn.prepare("SELECT id, expires_unix FROM asks WHERE status = 'pending' AND expires_unix <= ?1")?;
    let due: Vec<(String, i64)> = stmt
        .query_map(params![now], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (id, expires_unix) in due {
        let changed = conn.execute(
            "UPDATE asks SET status = 'expired' WHERE id = ?1 AND status = 'pending'",
            params![id],
        )?;
        if changed == 1 {
            conn.execute(
                "INSERT INTO ask_events (ask_id, kind, at_unix, detail) VALUES (?1, 'expired', ?2, NULL)",
                params![id, expires_unix],
            )?;
        }
    }
    Ok(())
}

/// One ask by id, any tenant (the signed page's lookup), expiry applied.
pub async fn find(state: &AppState, id: &str) -> Result<Option<AskRow>, AppError> {
    let id = id.to_string();
    state
        .db
        .with_conn(move |conn| {
            expire_due(conn, now_unix())?;
            conn.query_row(&format!("SELECT {ROW_COLUMNS} FROM asks WHERE id = ?1"), params![id], row_from)
                .optional()
                .map_err(AppError::from)
        })
        .await
}

/// One ask by id for `tenant`; unknown or another tenant's is `ask_not_found`.
async fn find_for_tenant(state: &AppState, tenant: &Tenant, id: &str) -> Result<AskRow, AppError> {
    match find(state, id).await? {
        Some(row) if row.tenant_id == tenant.id => Ok(row),
        _ => Err(ask_not_found(id)),
    }
}

fn ask_not_found(id: &str) -> AppError {
    AppError::Structured {
        code: ASK_NOT_FOUND,
        message: format!("no ask '{id}' for this tenant"),
        data: json!({"ask_id": id, "next_call": "host.ask.create"}),
    }
}

// ---- argument parsing ---------------------------------------------------------

fn invalid(msg: String) -> AppError {
    AppError::InvalidArgs(msg)
}

struct CreateArgs {
    question: String,
    options: Vec<String>,
    ttl_s: i64,
    context: Option<String>,
}

fn parse_create(args: &Value) -> Result<CreateArgs, AppError> {
    let question = args
        .get("question")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .ok_or_else(|| invalid("missing required argument 'question'".into()))?
        .to_string();
    if question.chars().count() > QUESTION_MAX_CHARS {
        return Err(invalid(format!("question: at most {QUESTION_MAX_CHARS} characters")));
    }
    let raw = args
        .get("options")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("missing required argument 'options' (an array of strings)".into()))?;
    if !(OPTIONS_MIN..=OPTIONS_MAX).contains(&raw.len()) {
        return Err(invalid(format!("options: {OPTIONS_MIN} to {OPTIONS_MAX} entries, got {}", raw.len())));
    }
    let mut options: Vec<String> = Vec::with_capacity(raw.len());
    for v in raw {
        let s = v
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| invalid("options: every entry must be a non-empty string".into()))?;
        if s.chars().count() > OPTION_MAX_CHARS {
            return Err(invalid(format!("options: each entry at most {OPTION_MAX_CHARS} characters")));
        }
        if options.iter().any(|o| o == s) {
            return Err(invalid(format!("options: '{s}' appears twice")));
        }
        options.push(s.to_string());
    }
    let ttl_s = match args.get("ttl_s") {
        None | Some(Value::Null) => TTL_DEFAULT_S,
        Some(v) => v
            .as_i64()
            .filter(|t| (1..=TTL_MAX_S).contains(t))
            .ok_or_else(|| invalid(format!("ttl_s: an integer from 1 to {TTL_MAX_S}")))?,
    };
    let context = match args.get("context") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let c = v.as_str().ok_or_else(|| invalid("context: must be a string".into()))?;
            if c.len() > CONTEXT_MAX_BYTES {
                return Err(invalid(format!("context: at most {CONTEXT_MAX_BYTES} bytes")));
            }
            Some(c.to_string())
        }
    };
    Ok(CreateArgs { question, options, ttl_s, context })
}

// ---- host.ask.create ------------------------------------------------------------

/// `host.ask.create {question, options, ttl_s?, context?, notify?}` ->
/// `{ask_id, url, expires_unix, next_call, next_args, notified}`.
pub async fn create(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let parsed = parse_create(args)?;
    let id = new_ulid().to_ascii_lowercase();
    let now = now_unix();
    let expires_unix = now + parsed.ttl_s;
    let options_json = serde_json::to_string(&parsed.options).map_err(|e| AppError::Internal(e.to_string()))?;

    let plan = state.plans.get(&tenant.plan).ok_or_else(|| {
        AppError::Internal(format!("tenant's plan '{}' is not in the loaded plan catalog", tenant.plan))
    })?;
    let open_max = plan.asks_open_max;

    let (tenant_id, row_id) = (tenant.id, id.clone());
    state
        .db
        .with_conn(move |conn| {
            // Counted and inserted on the one connection, so two concurrent
            // creates cannot both slip under the ceiling.
            expire_due(conn, now)?;
            prune_old(conn, now)?;
            let open: i64 = conn.query_row(
                "SELECT COUNT(*) FROM asks WHERE tenant_id = ?1 AND status = 'pending'",
                params![tenant_id],
                |r| r.get(0),
            )?;
            if open >= open_max {
                return Err(asks_limit(open_max, open));
            }
            conn.execute(
                "INSERT INTO asks (id, tenant_id, question, options_json, context, status, \
                                   created_unix, expires_unix, channel) \
                 VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6, ?7, 'link')",
                params![row_id, tenant_id, parsed.question, options_json, parsed.context, now, expires_unix],
            )?;
            conn.execute(
                "INSERT INTO ask_events (ask_id, kind, at_unix, detail) VALUES (?1, 'created', ?2, NULL)",
                params![row_id, now],
            )?;
            Ok(())
        })
        .await?;

    Ok(json!({
        "ask_id": id,
        "url": signed_ask_url(&state.public_url, &tenant.key_hash, &id, expires_unix),
        "expires_unix": expires_unix,
        "next_call": "host.ask.wait",
        "next_args": {"ask_id": id},
        // `notify: true` calls host.notify.send once that verb is live; it is
        // not, so the field is accepted and honestly reported unsent.
        "notified": false,
    }))
}

/// PRD-mcphost-elicitation-with-ask-fallback R3/R5: the audit row of a
/// question the client's own form answered (`channel: client`): already
/// `answered` (or `declined`/`cancelled`, recorded as the choice), never
/// pending, with its `created` and `answered` events.
pub async fn record_client_elicit(
    state: &AppState,
    tenant: &Tenant,
    message: &str,
    schema: &Value,
    choice: &str,
) -> Result<String, AppError> {
    let id = new_ulid().to_ascii_lowercase();
    let now = now_unix();
    let question: String = message.chars().take(QUESTION_MAX_CHARS).collect();
    let context = schema.to_string();
    let (tenant_id, row_id, choice) = (tenant.id, id.clone(), choice.to_string());
    state
        .db
        .with_conn(move |conn| {
            conn.execute(
                "INSERT INTO asks (id, tenant_id, question, options_json, context, status, choice, \
                                   created_unix, expires_unix, answered_unix, channel) \
                 VALUES (?1, ?2, ?3, '[]', ?4, 'answered', ?5, ?6, ?6, ?6, 'client')",
                params![row_id, tenant_id, question, context, choice, now],
            )?;
            conn.execute(
                "INSERT INTO ask_events (ask_id, kind, at_unix, detail) VALUES (?1, 'created', ?2, NULL)",
                params![row_id, now],
            )?;
            conn.execute(
                "INSERT INTO ask_events (ask_id, kind, at_unix, detail) VALUES (?1, 'answered', ?2, ?3)",
                params![row_id, now, choice],
            )?;
            Ok(())
        })
        .await?;
    Ok(id)
}

// ---- host.ask.wait ----------------------------------------------------------------

/// `host.ask.wait {ask_id, timeout_s?}`: blocks until the ask is answered,
/// expired, or `timeout_s` (default [`WAIT_DEFAULT_S`], clamped
/// `1..=`[`WAIT_MAX_S`]) elapses.
pub async fn wait(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let ask_id = args
        .get("ask_id")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("missing required argument 'ask_id'".into()))?
        .to_string();
    let timeout_s = args.get("timeout_s").and_then(Value::as_i64).unwrap_or(WAIT_DEFAULT_S).clamp(1, WAIT_MAX_S);
    let started = Instant::now();
    let deadline = started + Duration::from_secs(timeout_s as u64);
    let n = notifier(&ask_id);
    let result = wait_loop(state, tenant, &ask_id, &n, deadline).await;
    release_notifier(&ask_id, n);
    let waited_ms = started.elapsed().as_millis() as i64;
    let (id, tenant_id) = (ask_id.clone(), tenant.id);
    let _ = state
        .db
        .with_conn(move |conn| {
            conn.execute(
                "UPDATE asks SET waited_total_ms = waited_total_ms + ?1 WHERE id = ?2 AND tenant_id = ?3",
                params![waited_ms, id, tenant_id],
            )?;
            Ok(())
        })
        .await;
    match result? {
        Some(row) => Ok(wait_result(&row, waited_ms)),
        None => Ok(json!({
            "ask_id": ask_id,
            "status": "pending",
            "waited_ms": waited_ms,
            "next_call": "host.ask.wait",
            "next_args": {"ask_id": ask_id},
        })),
    }
}

/// `Ok(Some(row))` once answered, `Ok(None)` at the deadline still pending,
/// `Err(ask_expired)` when it lapsed.
async fn wait_loop(
    state: &AppState,
    tenant: &Tenant,
    ask_id: &str,
    n: &Arc<Notify>,
    deadline: Instant,
) -> Result<Option<AskRow>, AppError> {
    loop {
        // Arm the wake-up BEFORE reading the row: an answer landing between
        // the read and the sleep is then never missed.
        let notified = n.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let row = find_for_tenant(state, tenant, ask_id).await?;
        match row.status.as_str() {
            "answered" => return Ok(Some(row)),
            "expired" => return Err(ask_expired(&row)),
            _ => {}
        }
        let now = Instant::now();
        if now >= deadline {
            return Ok(None);
        }
        let nap = REREAD_EVERY.min(deadline - now);
        let _ = tokio::time::timeout(nap, notified.as_mut()).await;
    }
}

fn wait_result(row: &AskRow, waited_ms: i64) -> Value {
    json!({
        "ask_id": row.id,
        "status": "answered",
        "choice": row.choice,
        "answered_unix": row.answered_unix,
        "waited_ms": waited_ms,
    })
}

fn asks_limit(limit: i64, used: i64) -> AppError {
    AppError::Structured {
        code: ASKS_LIMIT,
        message: format!("{used} asks are already pending; this plan allows {limit} (asks_open_max)"),
        data: json!({
            "limit": limit,
            "used": used,
            "asks_open_max": limit,
            "next_call": "host.ask.list",
            "next_args": {"status": "pending"},
        }),
    }
}

fn ask_expired(row: &AskRow) -> AppError {
    AppError::Structured {
        code: ASK_EXPIRED,
        message: format!("ask '{}' expired at {} with no answer; do not act on a missing answer", row.id, row.expires_unix),
        data: json!({
            "ask_id": row.id,
            "expires_unix": row.expires_unix,
            "next_call": "host.ask.create",
        }),
    }
}

// ---- host.ask.get / host.ask.list / retention ---------------------------------------

/// Days an ask row (and its events) survives past its expiry.
pub const RETENTION_DAYS: i64 = 30;

/// `host.ask.list` default and maximum page size.
pub const LIST_DEFAULT: i64 = 50;
pub const LIST_MAX: i64 = 100;

/// The audit row of one ask: `{ask_id, question, options, status, choice,
/// created_unix, answered_unix, expires_unix, waited_total_ms}`.
fn audit_row(row: &AskRow) -> Value {
    json!({
        "ask_id": row.id,
        "question": row.question,
        "options": row.options,
        "status": row.status,
        "choice": row.choice,
        "created_unix": row.created_unix,
        "answered_unix": row.answered_unix,
        "expires_unix": row.expires_unix,
        "waited_total_ms": row.waited_total_ms,
        "channel": row.channel,
    })
}

/// Deletes asks (and their events) that expired more than
/// [`RETENTION_DAYS`] ago.
pub(crate) fn prune_old(conn: &Connection, now: i64) -> Result<(), AppError> {
    let cutoff = now - RETENTION_DAYS * 86_400;
    conn.execute("DELETE FROM ask_events WHERE ask_id IN (SELECT id FROM asks WHERE expires_unix < ?1)", params![cutoff])?;
    conn.execute("DELETE FROM asks WHERE expires_unix < ?1", params![cutoff])?;
    Ok(())
}

/// `host.ask.get {ask_id}`: the audit row plus its `events`
/// (`created`, then one `answered` or `expired`).
pub async fn get(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let ask_id = args
        .get("ask_id")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("missing required argument 'ask_id'".into()))?
        .to_string();
    let row = find_for_tenant(state, tenant, &ask_id).await?;
    let id = ask_id.clone();
    let events: Vec<Value> = state
        .db
        .with_conn(move |conn| {
            let mut stmt = conn.prepare("SELECT kind, at_unix, detail FROM ask_events WHERE ask_id = ?1 ORDER BY id")?;
            let rows = stmt
                .query_map(params![id], |r| {
                    let (kind, at_unix, detail): (String, i64, Option<String>) = (r.get(0)?, r.get(1)?, r.get(2)?);
                    // An `answered` event's detail is the choice; `created` and
                    // `expired` carry none.
                    Ok(match detail {
                        Some(d) => json!({"kind": kind, "at_unix": at_unix, "choice": d}),
                        None => json!({"kind": kind, "at_unix": at_unix}),
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await?;
    let mut out = audit_row(&row);
    out["events"] = json!(events);
    Ok(out)
}

/// The `status` values of `host.ask.list`: the descriptor's enum and
/// [`list`]'s own check read this one list.
pub const STATUSES: &[&str] = &["pending", "answered", "expired"];

/// `host.ask.list {status?, limit?}`: this tenant's asks, newest first.
pub async fn list(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let status = match args.get("status") {
        None | Some(Value::Null) => None,
        Some(v) => match v.as_str() {
            Some(s) if STATUSES.contains(&s) => Some(s.to_string()),
            _ => return Err(invalid(format!("status: one of {}", STATUSES.join(", ")))),
        },
    };
    let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(LIST_DEFAULT).clamp(1, LIST_MAX);
    let tenant_id = tenant.id;
    let rows = state
        .db
        .with_conn(move |conn| {
            let now = now_unix();
            expire_due(conn, now)?;
            prune_old(conn, now)?;
            let mut stmt = conn.prepare(&format!(
                "SELECT {ROW_COLUMNS} FROM asks WHERE tenant_id = ?1 AND (?2 IS NULL OR status = ?2) \
                 ORDER BY created_unix DESC, id DESC LIMIT ?3"
            ))?;
            let rows = stmt
                .query_map(params![tenant_id, status, limit], row_from)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await?;
    Ok(json!({"asks": rows.iter().map(audit_row).collect::<Vec<_>>()}))
}

// ---- the answer page: GET/POST /a/{id} ------------------------------------------

/// Outcome of one answer attempt, decided inside a single database call.
enum Answered {
    /// This request's choice is now the recorded answer.
    Recorded { answered_unix: i64 },
    /// An earlier request already answered; its choice stands.
    Already { choice: String, answered_unix: i64 },
    /// Expired or unknown between the signature check and the write.
    Gone,
}

/// Records `choice` iff the ask is still pending and unexpired. The guarded
/// `UPDATE ... WHERE status = 'pending'` is the whole first-answer-wins
/// rule: the database serialises concurrent attempts and exactly one sees
/// `changed == 1`.
fn record_answer(conn: &Connection, id: &str, choice: &str, ip_hash: &str, now: i64) -> Result<Answered, AppError> {
    expire_due(conn, now)?;
    let changed = conn.execute(
        "UPDATE asks SET status = 'answered', choice = ?2, answered_unix = ?3, ip_hash = ?4 \
         WHERE id = ?1 AND status = 'pending' AND expires_unix > ?3",
        params![id, choice, now, ip_hash],
    )?;
    if changed == 1 {
        conn.execute(
            "INSERT INTO ask_events (ask_id, kind, at_unix, detail) VALUES (?1, 'answered', ?2, ?3)",
            params![id, now, choice],
        )?;
        return Ok(Answered::Recorded { answered_unix: now });
    }
    let row: Option<(String, Option<String>, Option<i64>)> = conn
        .query_row("SELECT status, choice, answered_unix FROM asks WHERE id = ?1", params![id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()?;
    Ok(match row {
        Some((status, Some(choice), Some(answered_unix))) if status == "answered" => {
            Answered::Already { choice, answered_unix }
        }
        _ => Answered::Gone,
    })
}

/// The one response every unsigned, mis-signed, expired or unknown link gets:
/// byte-identical to the router's own no-route 404, leaking nothing about
/// which of those it was.
fn not_found() -> Response {
    StatusCode::NOT_FOUND.into_response()
}

/// The ask behind a link iff the link is genuine and still live.
async fn resolve_link(state: &AppState, id: &str, params: &std::collections::HashMap<String, String>) -> Option<AskRow> {
    let row = find(state, id).await.ok()??;
    let tenant = state.db.find_tenant_by_id(row.tenant_id).await.ok()??;
    let exp: i64 = params.get("exp")?.parse().ok()?;
    let sig = params.get("sig")?;
    if !constant_time_eq(sig.as_bytes(), sign(&tenant.key_hash, id, exp).as_bytes()) {
        return None;
    }
    // The signed expiry is the ask's own; a link for an older/other expiry
    // or a lapsed ask is the same 404.
    (exp == row.expires_unix && now_unix() < exp && row.status != "expired").then_some(row)
}

fn source_ip_hash(headers: &HeaderMap, peer: SocketAddr) -> String {
    hash_key(&crate::claim::source_ip(headers, peer))
}

fn page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>{}</title></head><body style=\"font-family:system-ui,sans-serif;max-width:32rem;margin:2rem auto;padding:0 1rem\">{body}</body></html>",
        html_escape(title)
    )
}

/// `context` is markdown with raw HTML escaped: escaping first and then
/// only splitting paragraphs/lines can never reintroduce a tag.
fn render_context(context: &str) -> String {
    context
        .split("\n\n")
        .filter(|p| !p.trim().is_empty())
        .map(|p| format!("<p>{}</p>", html_escape(p.trim()).replace('\n', "<br>")))
        .collect()
}

fn expires_in(expires_unix: i64, now: i64) -> String {
    let mins = ((expires_unix - now).max(0) + 59) / 60;
    format!("expires in {mins} min")
}

fn recorded_at(answered_unix: i64) -> String {
    let secs = answered_unix.rem_euclid(86_400);
    format!("recorded at {:02}:{:02} UTC", secs / 3600, (secs % 3600) / 60)
}

fn render_question(row: &AskRow, query: &str, now: i64) -> String {
    let mut body = format!("<h1>{}</h1>", html_escape(&row.question));
    if let Some(context) = &row.context {
        body.push_str(&render_context(context));
    }
    if row.status == "answered" {
        body.push_str(&format!(
            "<p>Recorded: <strong>{}</strong> ({}).</p>",
            html_escape(row.choice.as_deref().unwrap_or_default()),
            recorded_at(row.answered_unix.unwrap_or_default())
        ));
        return page("Answered", &body);
    }
    body.push_str(&format!("<p>{}</p>", html_escape(&expires_in(row.expires_unix, now))));
    body.push_str(&format!("<form method=\"post\" action=\"{}?{}\">", ask_path(&row.id), html_escape(query)));
    for option in &row.options {
        let label = html_escape(option);
        body.push_str(&format!(
            "<p><button type=\"submit\" name=\"choice\" value=\"{label}\" style=\"font-size:1.1rem;padding:.6rem 1.2rem\">{label}</button></p>"
        ));
    }
    body.push_str("</form>");
    page("Question", &body)
}

/// `GET /a/{id}?exp&sig`: the question and one button per option. No key.
pub async fn get_page(
    State(state): State<std::sync::Arc<AppState>>,
    Path(id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let Some(row) = resolve_link(&state, &id, &params).await else {
        return not_found();
    };
    let query = format!("exp={}&sig={}", params["exp"], params["sig"]);
    html_response(StatusCode::OK, render_question(&row, &query, now_unix()))
}

/// The submitted `choice`, from a form body or a JSON object.
fn submitted_choice(body: &Bytes) -> Option<String> {
    if let Ok(Value::Object(o)) = serde_json::from_slice::<Value>(body) {
        return o.get("choice").and_then(Value::as_str).map(str::to_string);
    }
    let uri: axum::http::Uri = format!("/?{}", String::from_utf8_lossy(body)).parse().ok()?;
    let Query(map) = Query::<std::collections::HashMap<String, String>>::try_from_uri(&uri).ok()?;
    map.get("choice").cloned()
}

fn wants_html(headers: &HeaderMap) -> bool {
    headers
        .get("accept")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|a| a.contains("text/html"))
}

fn error_response(status: StatusCode, code: &str, extra: Value, headers: &HeaderMap, html: String) -> Response {
    if wants_html(headers) {
        return html_response(status, html);
    }
    let mut error = json!({"code": code});
    if let (Some(e), Some(x)) = (error.as_object_mut(), extra.as_object()) {
        e.extend(x.clone());
    }
    (status, axum::Json(json!({"ok": false, "error": error}))).into_response()
}

/// `POST /a/{id}?exp&sig` with `choice`: the first answer wins (200); a
/// later one is `409 ask_answered` naming the recorded choice; a `choice`
/// outside the options is `400 choice_invalid`. Never reads a tenant key.
#[allow(clippy::too_many_arguments)] // axum extractors, one each
pub async fn post_answer(
    State(state): State<std::sync::Arc<AppState>>,
    Path(id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let ip_hash = source_ip_hash(&headers, peer);
    if !state.url_rate_limiter.allow(&format!("ask:{ip_hash}"), crate::state::PUBLIC_URL_RATE_LIMIT_PER_MINUTE) {
        let mut response = (
            StatusCode::TOO_MANY_REQUESTS,
            axum::Json(json!({"ok": false, "error": {"code": "rate_limited"}})),
        )
            .into_response();
        response.headers_mut().insert("Retry-After", axum::http::HeaderValue::from_static("60"));
        return response;
    }
    let Some(row) = resolve_link(&state, &id, &params).await else {
        return not_found();
    };
    let Some(choice) = submitted_choice(&body).filter(|c| row.options.contains(c)) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "choice_invalid",
            json!({"options": row.options}),
            &headers,
            page("Pick one", "<h1>Pick one of the offered options</h1>"),
        );
    };
    let (ask_id, picked, hash) = (id.clone(), choice.clone(), ip_hash);
    let outcome = state.db.with_conn(move |conn| record_answer(conn, &ask_id, &picked, &hash, now_unix())).await;
    match outcome {
        Ok(Answered::Recorded { answered_unix }) => {
            wake(&id);
            html_response(
                StatusCode::OK,
                page(
                    "Recorded",
                    &format!(
                        "<h1>Recorded: {}</h1><p>{}.</p>",
                        html_escape(&choice),
                        html_escape(&recorded_at(answered_unix))
                    ),
                ),
            )
        }
        Ok(Answered::Already { choice: first, answered_unix }) => error_response(
            StatusCode::CONFLICT,
            "ask_answered",
            json!({"choice": first, "answered_unix": answered_unix}),
            &headers,
            page(
                "Already answered",
                &format!(
                    "<h1>ask_answered</h1><p>Already answered: <strong>{}</strong> ({}).</p>",
                    html_escape(&first),
                    html_escape(&recorded_at(answered_unix))
                ),
            ),
        ),
        Ok(Answered::Gone) => not_found(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "storage error").into_response(),
    }
}

crate::err_code! {
    pub ASK_EXPIRED,
    code: "ask_expired", family: Share, class: AgentFault, retryable: false,
    remedy: "Do not act on a missing answer; create a new ask with host.ask.create.",
}

crate::err_code! {
    pub ASK_NOT_FOUND,
    code: "ask_not_found", family: Share, class: AgentFault, retryable: false,
    remedy: "List asks with host.ask.list and use an id from data.next_call's result.",
}

crate::err_code! {
    pub ASKS_LIMIT,
    code: "asks_limit", family: Plan, class: PlanLimit, retryable: false,
    remedy: "Wait for a pending ask to be answered or expire, or reuse one from host.ask.list; data.asks_open_max is the ceiling.",
}

