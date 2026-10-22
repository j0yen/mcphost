//! PRD-mcphost-sync-pull-with-change-detection: the pull of one sync.
//! [`fetch`] dispatches a [`SourceKind`] to its source (an exhaustive `match`,
//! so a kind without an implementation does not compile) and returns a
//! [`Snapshot`]; change is detected at three levels -- the provider's
//! `ETag`/`Last-Modified` (a 304 costs one call and no parse), a whole-body
//! hash, then a per-row hash keyed on `primary_key` inside
//! [`crate::tables::upsert_rows_sync`] -- so only added, updated and deleted
//! rows are written. [`execute`] turns one fetch into an [`Outcome`] the run
//! stores as the sync's receipt.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use super::{SourceKind, SyncRow};
use crate::errors::AppError;
use crate::state::AppState;

/// R3: a fetch that takes longer than this fails the run.
pub const FETCH_TIMEOUT_S: u64 = 30;
/// R6: the most bytes one source response may carry.
pub const BODY_CAP_BYTES: usize = 10 * 1024 * 1024;
/// R4: the pause when a 429 names no `Retry-After`.
pub const RETRY_AFTER_DEFAULT_S: i64 = 60;
/// R4: the longest pause a `Retry-After` may ask for.
pub const RETRY_AFTER_CAP_S: i64 = 3600;
/// Paginated sources stop after this many pages (100 rows each).
pub const MAX_PAGES: usize = 100;

/// `status` values a receipt carries.
pub const STATUS_RUNNING: &str = "running";
pub const STATUS_OK: &str = "ok";
pub const STATUS_ERROR: &str = "error";
pub const STATUS_RATE_LIMITED: &str = "rate_limited";
pub const STATUS_AUTH_EXPIRED: &str = "auth_expired";
pub const STATUS_SUSPICIOUS_EMPTY: &str = "suspicious_empty";
pub const STATUS_DEFERRED: &str = "deferred";

/// One source row: column name to `String` (or `Null`) cell.
pub type Row = Map<String, Value>;

/// What the next pull sends back to the provider and compares against.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cursor {
    pub etag: Option<String>,
    pub modified: Option<String>,
    pub body_hash: Option<String>,
}

/// One fetched dataset. `unchanged` is set when the provider answered 304 or
/// the body hash matched the cursor's, in which case `rows` is empty.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub rows: Vec<Row>,
    pub etag: Option<String>,
    pub modified: Option<String>,
    pub body_hash: Option<String>,
    pub bytes: i64,
    pub unchanged: bool,
    pub provider_calls: i64,
}

/// Why a fetch produced no snapshot.
#[derive(Debug)]
pub enum SourceError {
    /// The provider answered a non-success status.
    Http { status: u16, retry_after_s: Option<i64> },
    /// Anything else: unreachable, too large, unparseable.
    Failed(String),
}

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SourceError::Http { status, .. } => write!(f, "upstream returned HTTP {status}"),
            SourceError::Failed(msg) => f.write_str(msg),
        }
    }
}

/// What a source needs to fetch: the client, the sync's `ref`, the connected
/// account's access token (OAuth kinds), and an API base that replaces the
/// provider's own (a fixture server).
pub struct SourceCtx<'a> {
    pub client: &'a reqwest::Client,
    pub reference: &'a str,
    pub token: Option<&'a str>, // allowlist: HLT-010 false positive -- SourceCtx has no Debug impl; the audit awk leaves pending=1 after derive(Debug) on enum SourceError (:66); rules fix = mcphost hand PR (awk pending); bearer secret, never print
    pub api_base: Option<&'a str>,
}

/// The dispatch from a kind to its source. Exhaustive: a kind added to
/// [`SourceKind::ALL`] without an arm here does not compile.
pub async fn fetch(kind: SourceKind, ctx: &SourceCtx<'_>, cursor: &Cursor) -> Result<Snapshot, SourceError> {
    match kind {
        SourceKind::CsvUrl => super::sources::csv_url::fetch(ctx, cursor).await,
        SourceKind::GoogleSheet => super::sources::google_sheet::fetch(ctx, cursor).await,
        SourceKind::Airtable => super::sources::airtable::fetch(ctx, cursor).await,
        SourceKind::NotionDb => super::sources::notion_db::fetch(ctx, cursor).await,
        SourceKind::HttpJson => super::sources::http_json::fetch(ctx, cursor).await,
    }
}

// ---- shared fetch helpers --------------------------------------------------------

/// Lowercase hex sha256 of `bytes`.
pub fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// The `Retry-After` header as whole seconds (an HTTP-date form is ignored).
fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<i64> {
    headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|s| *s >= 0)
}

fn header_string(headers: &reqwest::header::HeaderMap, name: reqwest::header::HeaderName) -> Option<String> {
    headers.get(name)?.to_str().ok().map(str::to_string)
}

/// One response of a source request.
pub(crate) struct Fetched {
    pub not_modified: bool,
    pub body: Vec<u8>,
    pub etag: Option<String>,
    pub modified: Option<String>,
}

/// Sends `req`, maps a non-success status to [`SourceError::Http`], and reads
/// the body up to [`BODY_CAP_BYTES`]. A 304 comes back as `not_modified`.
pub(crate) async fn send_capped(req: reqwest::RequestBuilder) -> Result<Fetched, SourceError> {
    let mut resp = req.send().await.map_err(|e| SourceError::Failed(format!("upstream unreachable: {e}")))?;
    let status = resp.status();
    let etag = header_string(resp.headers(), reqwest::header::ETAG);
    let modified = header_string(resp.headers(), reqwest::header::LAST_MODIFIED);
    if status == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(Fetched { not_modified: true, body: Vec::new(), etag, modified });
    }
    if !status.is_success() {
        return Err(SourceError::Http { status: status.as_u16(), retry_after_s: retry_after(resp.headers()) });
    }
    if resp.content_length().is_some_and(|n| n > BODY_CAP_BYTES as u64) {
        return Err(SourceError::Failed(format!("source body exceeds the {BODY_CAP_BYTES} byte cap")));
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| SourceError::Failed(format!("reading the source body: {e}")))?
    {
        if body.len() + chunk.len() > BODY_CAP_BYTES {
            return Err(SourceError::Failed(format!("source body exceeds the {BODY_CAP_BYTES} byte cap")));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Fetched { not_modified: false, body, etag, modified })
}

/// The snapshot shell of one single-response fetch: `etag`/`modified`/`bytes`
/// and the body hash set, `unchanged` when the provider answered 304 or the
/// hash equals the cursor's (so the caller skips parsing).
pub(crate) fn snapshot_of(fetched: &Fetched, cursor: &Cursor) -> Snapshot {
    let mut snap = Snapshot {
        etag: fetched.etag.clone(),
        modified: fetched.modified.clone(),
        provider_calls: 1,
        ..Snapshot::default()
    };
    if fetched.not_modified {
        snap.unchanged = true;
        return snap;
    }
    snap.bytes = fetched.body.len() as i64;
    let hash = hex_sha256(&fetched.body);
    snap.unchanged = cursor.body_hash.as_deref() == Some(hash.as_str());
    snap.body_hash = Some(hash);
    snap
}

/// Adds the conditional-request headers a cursor earns.
pub(crate) fn conditional(req: reqwest::RequestBuilder, cursor: &Cursor) -> reqwest::RequestBuilder {
    let req = match &cursor.etag {
        Some(etag) => req.header(reqwest::header::IF_NONE_MATCH, etag),
        None => req,
    };
    match &cursor.modified {
        Some(m) => req.header(reqwest::header::IF_MODIFIED_SINCE, m),
        None => req,
    }
}

/// A column name the table can hold: invalid characters become `_`, a leading
/// digit gets a `_` prefix, the `_sync_` prefix is reserved, 64 chars at most.
pub fn sanitize_column(raw: &str) -> String {
    let mut name: String = raw
        .trim()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect();
    if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
        name.insert(0, '_');
    }
    if name.starts_with("_sync_") {
        name.insert(0, 'x');
    }
    name.truncate(64);
    name
}

/// Sanitizes `raw` headers and makes them unique (`name`, `name_2`, ...).
pub fn unique_columns(raw: &[String]) -> Vec<String> {
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    raw.iter()
        .map(|r| {
            let base = sanitize_column(r);
            let n = seen.entry(base.clone()).or_insert(0);
            *n += 1;
            if *n == 1 { base } else { format!("{base}_{n}") }
        })
        .collect()
}

/// A JSON cell as table text: strings as-is, null stays null, anything else
/// as its JSON text.
pub fn cell_text(v: &Value) -> Value {
    match v {
        Value::Null => Value::Null,
        Value::String(_) => v.clone(),
        other => Value::String(other.to_string()),
    }
}

// ---- the run ---------------------------------------------------------------------

/// How a pull treats unchanged data. `change_detection: false` is the naive
/// full rewrite AC 1 measures against: no cursor is sent and every row of the
/// snapshot is written.
#[derive(Debug, Clone, Copy)]
pub struct RunOptions {
    pub change_detection: bool,
}

impl Default for RunOptions {
    fn default() -> Self {
        RunOptions { change_detection: true }
    }
}

/// The result of one pull, stored as the sync's receipt.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub status: &'static str,
    pub rows_added: i64,
    pub rows_updated: i64,
    pub rows_deleted: i64,
    pub bytes: i64,
    pub change_detected: bool,
    pub error: Option<String>,
    pub provider_calls: i64,
    /// A 429's pause, already clamped.
    pub retry_after_s: Option<i64>,
    /// The sync stops being scheduled (`auth_expired`).
    pub pause: bool,
    /// The cursor and snapshot row count to store (`ok` only).
    pub cursor: Option<(Cursor, i64)>,
}

impl Outcome {
    fn new(status: &'static str) -> Self {
        Outcome {
            status,
            rows_added: 0,
            rows_updated: 0,
            rows_deleted: 0,
            bytes: 0,
            change_detected: false,
            error: None,
            provider_calls: 0,
            retry_after_s: None,
            pause: false,
            cursor: None,
        }
    }

    fn failed(error: impl Into<String>, provider_calls: i64) -> Self {
        let mut o = Outcome::new(STATUS_ERROR);
        o.error = Some(error.into());
        o.provider_calls = provider_calls;
        o
    }

    /// Rows written: what AC 1 counts.
    pub fn writes(&self) -> i64 {
        self.rows_added + self.rows_updated + self.rows_deleted
    }
}

/// The access token of the tenant-level connection `kind` needs, or `None`
/// when the kind needs none. `Err(())` is a missing or revoked connection.
async fn access_token(state: &AppState, tenant_id: i64, kind: SourceKind) -> Result<Option<String>, ()> {
    let Some(provider) = kind.needs_connection() else { return Ok(None) };
    let token = state
        .db
        .get_vault_token(tenant_id, provider.to_string(), crate::connect::flow::TENANT_SUBJECT.to_string())
        .await
        .map_err(|_| ())?
        .filter(|t| t.revoked_unix.is_none())
        .ok_or(())?;
    state.secrets.decrypt(&token.access_enc, &token.access_nonce).map(Some).map_err(|_| ())
}

async fn load_cursor(state: &AppState, sync_id: &str) -> Result<Cursor, AppError> {
    let id = sync_id.to_string();
    state
        .db
        .with_conn(move |conn| {
            use rusqlite::OptionalExtension;
            let cursor = conn
                .query_row(
                    "SELECT etag, modified, body_hash FROM sync_cursors WHERE sync_id = ?1",
                    rusqlite::params![id],
                    |r| Ok(Cursor { etag: r.get(0)?, modified: r.get(1)?, body_hash: r.get(2)? }),
                )
                .optional()?;
            Ok(cursor.unwrap_or_default())
        })
        .await
}

/// Fetches the sync's source and applies the snapshot to its table.
pub(super) async fn execute(state: &AppState, row: &SyncRow, opts: RunOptions) -> Outcome {
    let Some(kind) = SourceKind::parse(&row.kind) else {
        return Outcome::failed(format!("unknown source kind '{}'", row.kind), 0);
    };
    let token = match access_token(state, row.tenant_id, kind).await {
        Ok(t) => t,
        Err(()) => {
            let mut o = Outcome::new(STATUS_AUTH_EXPIRED);
            o.error = Some(format!("no live '{}' connection", kind.needs_connection().unwrap_or("provider")));
            o.pause = true;
            return o;
        }
    };
    let (own_domain, allow_loopback) = crate::index::url_policy(state);
    let client = match crate::kinds::http::sync_client(&own_domain, allow_loopback, Duration::from_secs(FETCH_TIMEOUT_S)) {
        Ok(c) => c,
        Err(e) => return Outcome::failed(format!("building the fetch client: {e:?}"), 0),
    };
    let cursor = if opts.change_detection {
        match load_cursor(state, &row.id).await {
            Ok(c) => c,
            Err(e) => return Outcome::failed(format!("reading the cursor: {e}"), 0),
        }
    } else {
        Cursor::default()
    };
    let api_base = super::sources::api_base_override(state, kind);
    let ctx = SourceCtx { client: &client, reference: &row.reference, token: token.as_deref(), api_base: api_base.as_deref() };
    let fetched = tokio::time::timeout(Duration::from_secs(FETCH_TIMEOUT_S), fetch(kind, &ctx, &cursor)).await;
    let snapshot = match fetched {
        Err(_) => return Outcome::failed(format!("fetch timed out after {FETCH_TIMEOUT_S} s"), 1),
        Ok(Err(SourceError::Http { status: 429, retry_after_s })) => {
            let mut o = Outcome::failed("upstream returned HTTP 429", 1);
            o.status = STATUS_RATE_LIMITED;
            o.retry_after_s = Some(retry_after_s.unwrap_or(RETRY_AFTER_DEFAULT_S).clamp(1, RETRY_AFTER_CAP_S));
            return o;
        }
        Ok(Err(SourceError::Http { status: status @ (401 | 403), .. })) if kind.needs_connection().is_some() => {
            let mut o = Outcome::failed(format!("upstream returned HTTP {status}"), 1);
            o.status = STATUS_AUTH_EXPIRED;
            o.pause = true;
            return o;
        }
        Ok(Err(e)) => return Outcome::failed(e.to_string(), 1),
        Ok(Ok(s)) => s,
    };

    let mut out = Outcome::new(STATUS_OK);
    out.bytes = snapshot.bytes;
    out.provider_calls = snapshot.provider_calls.max(1);
    let next_cursor = Cursor {
        etag: snapshot.etag.clone().or(cursor.etag.clone()),
        modified: snapshot.modified.clone().or(cursor.modified.clone()),
        body_hash: snapshot.body_hash.clone().or(cursor.body_hash.clone()),
    };
    if snapshot.unchanged && opts.change_detection {
        out.cursor = Some((next_cursor, -1));
        return out;
    }
    match apply(state, row, &snapshot, opts, &mut out).await {
        Ok(()) => {}
        Err(e) => return Outcome::failed(e.to_string(), out.provider_calls),
    }
    if out.status == STATUS_OK {
        out.cursor = Some((next_cursor, snapshot.rows.len() as i64));
    }
    out
}

/// Writes `snapshot` into the sync's table and fills the counters of `out`;
/// an empty snapshot over a non-empty table is `suspicious_empty`.
async fn apply(
    state: &AppState,
    row: &SyncRow,
    snapshot: &Snapshot,
    opts: RunOptions,
    out: &mut Outcome,
) -> Result<(), AppError> {
    let tenant = state
        .db
        .find_tenant_by_id(row.tenant_id)
        .await?
        .ok_or_else(|| AppError::Internal(format!("sync '{}' has no tenant", row.id)))?;
    let table = row.table.clone();
    let existing = {
        let (table, path) = (table.clone(), crate::tables::tenant_db_path(state, tenant.id));
        crate::tables::with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
            crate::tables::row_count_sync(conn, &table)
        })
        .await?
    };
    if snapshot.rows.is_empty() && existing > 0 {
        out.status = STATUS_SUSPICIOUS_EMPTY;
        out.error = Some(format!("the source returned no rows; the table holds {existing} and was left intact"));
        return Ok(());
    }
    let columns = snapshot_columns(&snapshot.rows);
    if let Some(pk) = &row.primary_key
        && !snapshot.rows.is_empty()
        && !columns.contains(pk)
    {
        return Err(AppError::InvalidArgs(format!(
            "primary_key '{pk}' is not a column of the source (columns: {})",
            columns.join(", ")
        )));
    }
    let keyed = key_rows(&snapshot.rows, &columns, row.primary_key.as_deref())?;
    let plan = state
        .plans
        .get(&tenant.plan)
        .ok_or_else(|| AppError::Internal(format!("tenant's plan '{}' is not in the loaded plan catalog", tenant.plan)))?;
    let rows_max = plan.table_rows_max;
    let (catalog, billing_mode, plan_name) = (state.plans.clone(), state.billing_config.billing_mode(), tenant.plan.clone());
    let over_quota = move |existing: i64| {
        crate::billing::quota_exceeded_for(&catalog, billing_mode, &plan_name, "table_rows_max", rows_max, existing, None)
    };
    let delete_missing = row.mode != super::Mode::Push.as_str();
    let force_rewrite = !opts.change_detection;
    let path = crate::tables::tenant_db_path(state, tenant.id);
    let counts = crate::tables::with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
        crate::tables::upsert_rows_sync(
            conn,
            &table,
            &columns,
            &keyed,
            crate::tables::UpsertMode { delete_missing, force_rewrite, rows_max, over_quota: &over_quota },
        )
    })
    .await?;
    out.rows_added = counts.added;
    out.rows_updated = counts.updated;
    out.rows_deleted = counts.deleted;
    out.change_detected = counts.added + counts.updated + counts.deleted > 0;
    if out.change_detected {
        if let Err(e) = state.db.mark_table_model_stale(tenant.id, row.table.clone()).await {
            tracing::warn!(error = %e, table = %row.table, "failed to mark table model stale after sync");
        }
        crate::tables::bump_change_counter(state, tenant.id, &row.table, counts.updated + counts.deleted > 0).await;
    }
    Ok(())
}

/// Every column the snapshot's rows carry, in first-seen order.
fn snapshot_columns(rows: &[Row]) -> Vec<String> {
    let mut cols: Vec<String> = Vec::new();
    for row in rows {
        for k in row.keys() {
            if !cols.contains(k) {
                cols.push(k.clone());
            }
        }
    }
    cols
}

/// R2: each row gets `_sync_key` (the `primary_key` cell, or the row's own
/// hash when none is declared) and `_sync_hash` (a hash of the whole row).
fn key_rows(rows: &[Row], columns: &[String], primary_key: Option<&str>) -> Result<Vec<crate::tables::KeyedRow>, AppError> {
    let mut out = Vec::with_capacity(rows.len());
    for (i, row) in rows.iter().enumerate() {
        let cells: Vec<Option<String>> = columns
            .iter()
            .map(|c| row.get(c).and_then(Value::as_str).map(str::to_string))
            .collect();
        let hash = hex_sha256(json!(cells).to_string().as_bytes());
        let key = match primary_key {
            Some(pk) => match row.get(pk).and_then(Value::as_str).filter(|s| !s.is_empty()) {
                Some(k) => k.to_string(),
                None => return Err(AppError::InvalidArgs(format!("row {} has no value for primary_key '{pk}'", i + 1))),
            },
            None => hash.clone(),
        };
        out.push(crate::tables::KeyedRow { key, hash, cells });
    }
    Ok(out)
}
