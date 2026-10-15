//! PRD-mcphost-call-audit-export: every call made with a tenant's keys is
//! listable (`host.calls.list`) and exportable (`host.calls.export`).
//!
//! `calls.credential` is one enum, [`Credential`]; every `INSERT INTO calls`
//! names the column (a test greps the sources and asserts it), and the write
//! path derives the value with [`resolve`] unless the caller scoped an
//! explicit one with [`scoped`] (webhook accepts, cron-driven background
//! ticks).

use std::future::Future;

use rusqlite::{ToSql, params_from_iter};
use serde_json::{Value, json};

use crate::db::{Db, Tenant};
use crate::errors::AppError;
use crate::state::{AppState, now_unix, parse_date_ymd_unix, rfc3339_from_unix};

/// The credential kind that made one call -- the closed set `calls.credential`
/// holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Credential {
    Primary,
    SubKey,
    Oauth,
    Trigger,
    Cron,
    Webhook,
    SharedCaller,
    Anonymous,
}

impl Credential {
    /// Every variant, in the order the PRD lists them.
    pub const ALL: &'static [Credential] = &[
        Credential::Primary,
        Credential::SubKey,
        Credential::Oauth,
        Credential::Trigger,
        Credential::Cron,
        Credential::Webhook,
        Credential::SharedCaller,
        Credential::Anonymous,
    ];

    /// The stored / wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Credential::Primary => "primary",
            Credential::SubKey => "sub_key",
            Credential::Oauth => "oauth",
            Credential::Trigger => "trigger",
            Credential::Cron => "cron",
            Credential::Webhook => "webhook",
            Credential::SharedCaller => "shared_caller",
            Credential::Anonymous => "anonymous",
        }
    }

    pub fn parse(s: &str) -> Option<Credential> {
        Credential::ALL.iter().copied().find(|c| c.as_str() == s)
    }
}

tokio::task_local! {
    /// An explicit credential for the `calls` row(s) a non-request path
    /// writes (a webhook accept, a cron-driven tick); read by
    /// [`current_override`] on the writer's own task, like
    /// `subkeys::CALL_KEY`.
    static CREDENTIAL_OVERRIDE: Credential;
}

/// Run `fut` with every `calls` row it writes stamped `credential`.
pub async fn scoped<F: Future>(credential: Credential, fut: F) -> F::Output {
    CREDENTIAL_OVERRIDE.scope(credential, fut).await
}

/// The scoped credential of the running task, if any.
pub fn current_override() -> Option<Credential> {
    CREDENTIAL_OVERRIDE.try_with(|c| *c).ok()
}

/// The credential a `calls` row is stamped with: an explicit [`scoped`] one,
/// else a sub-key id, else a cross-tenant caller, else the `auth_method` the
/// request authenticated with.
pub fn resolve(auth_method: Option<&str>, key_id: Option<i64>, caller_tenant_id: Option<i64>) -> Credential {
    if let Some(c) = current_override() {
        return c;
    }
    if key_id.is_some() {
        return Credential::SubKey;
    }
    if caller_tenant_id.is_some() {
        return Credential::SharedCaller;
    }
    match auth_method {
        Some("hosted_token") | Some("issuer_jwt") => Credential::Oauth,
        Some("anonymous") => Credential::Anonymous,
        _ => Credential::Primary,
    }
}

/// One listed call row, in the order the CSV export uses.
#[derive(Clone, Debug)]
pub struct CallRow {
    pub id: i64,
    pub ts: String,
    pub credential: String,
    pub key_label: Option<String>,
    pub tool: String,
    pub outcome: String,
    pub error_class: Option<String>,
    pub duration_ms: i64,
    pub bytes_in: i64,
    pub bytes_out: i64,
    pub end_user_subject: Option<String>,
    pub run_id: Option<String>,
}

impl CallRow {
    /// The row's field names, in CSV column order -- the one list the
    /// export header, the `fields` projection (`paged::CallsList::FIELDS`)
    /// and [`CallRow::to_json`] are all asserted against.
    pub const FIELDS: &'static [&'static str] = &[
        "ts",
        "credential",
        "key_label",
        "tool",
        "outcome",
        "error_class",
        "duration_ms",
        "bytes_in",
        "bytes_out",
        "end_user_subject",
        "run_id",
    ];

    pub fn to_json(&self) -> Value {
        json!({
            "ts": self.ts,
            "credential": self.credential,
            "key_label": self.key_label,
            "tool": self.tool,
            "outcome": self.outcome,
            "error_class": self.error_class,
            "duration_ms": self.duration_ms,
            "bytes_in": self.bytes_in,
            "bytes_out": self.bytes_out,
            "end_user_subject": self.end_user_subject,
            "run_id": self.run_id,
        })
    }
}

/// A resolved `[since, until]` window plus the optional filters.
#[derive(Clone, Debug, Default)]
pub struct CallFilter {
    pub since_unix: i64,
    pub until_unix: i64,
    pub key_label: Option<String>,
    pub tool: Option<String>,
    pub outcome: Option<String>,
    pub credential: Option<String>,
    /// R6: a sub-key sees only its own rows.
    pub only_key_id: Option<i64>,
}

/// One bound of the window: `-24h` / `-30d` (relative to now), an RFC 3339
/// instant, a `YYYY-MM-DD` day, or unix seconds.
pub fn parse_bound(raw: &str, now: i64) -> Option<i64> {
    let s = raw.trim();
    if let Some(rel) = s.strip_prefix('-') {
        let secs = crate::feedback::parse_since(rel)?;
        return Some(now - secs);
    }
    if let Ok(n) = s.parse::<i64>() {
        return Some(n);
    }
    if let Some(d) = parse_date_ymd_unix(s) {
        return Some(d);
    }
    chrono_free_rfc3339(s)
}

/// RFC 3339 `YYYY-MM-DDTHH:MM:SS[.f]Z` / `+00:00` as unix seconds.
fn chrono_free_rfc3339(s: &str) -> Option<i64> {
    let (date, rest) = s.split_once('T')?;
    let day = parse_date_ymd_unix(date)?;
    let time = rest.trim_end_matches('Z');
    let time = time.strip_suffix("+00:00").unwrap_or(time);
    let time = time.split('.').next()?;
    let mut parts = time.splitn(3, ':');
    let h: i64 = parts.next()?.parse().ok()?;
    let m: i64 = parts.next()?.parse().ok()?;
    let sec: i64 = parts.next().unwrap_or("0").parse().ok()?;
    Some(day + h * 3600 + m * 60 + sec)
}

fn bad_arg(field: &str, message: String) -> AppError {
    AppError::InvalidArgs(format!("{field}: {message}"))
}

/// The window and filters of one `host.calls.list` / `host.calls.export`
/// call.
pub fn filter_from_args(args: &Value, now: i64) -> Result<CallFilter, AppError> {
    let bound = |field: &str, default: i64| -> Result<i64, AppError> {
        match args.get(field) {
            None | Some(Value::Null) => Ok(default),
            Some(Value::String(s)) => parse_bound(s, now).ok_or_else(|| {
                bad_arg(field, format!("{field} must be like \"-24h\", \"2026-10-01\" or an RFC 3339 instant"))
            }),
            Some(Value::Number(n)) => n
                .as_i64()
                .ok_or_else(|| bad_arg(field, format!("{field} must be a string or unix seconds"))),
            Some(_) => Err(bad_arg(field, format!("{field} must be a string or unix seconds"))),
        }
    };
    let text = |field: &str| args.get(field).and_then(Value::as_str).map(str::to_string);
    let credential = text("credential");
    if let Some(c) = &credential
        && Credential::parse(c).is_none()
    {
        let valid: Vec<&str> = Credential::ALL.iter().map(|c| c.as_str()).collect();
        return Err(bad_arg("credential", format!("must be one of: {}", valid.join(", "))));
    }
    Ok(CallFilter {
        since_unix: bound("since", now - 86_400)?,
        until_unix: bound("until", now)?,
        key_label: text("key_label"),
        tool: text("tool"),
        outcome: text("outcome"),
        credential,
        only_key_id: None,
    })
}

const ROW_COLUMNS: &str = "c.id, c.started_at, c.credential, k.label, c.tool_name, \
     COALESCE(c.outcome, CASE c.ok WHEN 1 THEN 'ok' ELSE 'error' END), c.error_class, \
     c.duration_ms, c.bytes_in, c.bytes_out, c.end_user_subject, c.run_id";

fn row_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<CallRow> {
    Ok(CallRow {
        id: r.get(0)?,
        ts: r.get(1)?,
        credential: r.get(2)?,
        key_label: r.get(3)?,
        tool: r.get(4)?,
        outcome: r.get(5)?,
        error_class: r.get(6)?,
        duration_ms: r.get(7)?,
        bytes_in: r.get(8)?,
        bytes_out: r.get(9)?,
        end_user_subject: r.get(10)?,
        run_id: r.get(11)?,
    })
}

/// `WHERE` clause (and its bound values) shared by the list and the count.
fn where_clause(tenant_id: i64, f: &CallFilter) -> (String, Vec<Box<dyn ToSql + Send>>) {
    let mut sql = String::from("c.tenant_id = ? AND c.started_unix >= ? AND c.started_unix <= ?");
    let mut params: Vec<Box<dyn ToSql + Send>> = vec![
        Box::new(tenant_id),
        Box::new(f.since_unix),
        Box::new(f.until_unix),
    ];
    if let Some(v) = &f.key_label {
        sql.push_str(" AND k.label = ?");
        params.push(Box::new(v.clone()));
    }
    if let Some(v) = &f.tool {
        sql.push_str(" AND c.tool_name = ?");
        params.push(Box::new(v.clone()));
    }
    if let Some(v) = &f.outcome {
        sql.push_str(" AND COALESCE(c.outcome, CASE c.ok WHEN 1 THEN 'ok' ELSE 'error' END) = ?");
        params.push(Box::new(v.clone()));
    }
    if let Some(v) = &f.credential {
        sql.push_str(" AND c.credential = ?");
        params.push(Box::new(v.clone()));
    }
    if let Some(v) = f.only_key_id {
        sql.push_str(" AND c.key_id = ?");
        params.push(Box::new(v));
    }
    (sql, params)
}

impl Db {
    /// How many rows `f`'s window holds for `tenant_id`.
    pub async fn count_call_rows(&self, tenant_id: i64, f: CallFilter) -> Result<i64, AppError> {
        self.with_conn(move |conn| {
            let (clause, params) = where_clause(tenant_id, &f);
            let sql = format!(
                "SELECT COUNT(*) FROM calls c LEFT JOIN tenant_keys k ON k.id = c.key_id WHERE {clause}"
            );
            Ok(conn.query_row(&sql, params_from_iter(params.iter().map(|p| p.as_ref() as &dyn ToSql)), |r| r.get(0))?)
        })
        .await
    }

    /// Up to `limit` rows of the window, newest first (by `id`), resuming
    /// after `cursor` when given.
    pub async fn list_call_rows(
        &self,
        tenant_id: i64,
        f: CallFilter,
        cursor: Option<crate::paged::Cursor>,
        limit: i64,
    ) -> Result<Vec<CallRow>, AppError> {
        use crate::paged::{CallsList, CallsOrder, Paged, keyset_clause};
        self.with_conn(move |conn| {
            let (mut clause, mut params) = where_clause(tenant_id, &f);
            if let Some(c) = &cursor {
                let (keyset, _) = keyset_clause(CallsOrder::NewestFirst, c, 1);
                // `keyset_clause` numbers placeholders; this query uses bare `?`.
                clause.push_str(&format!(" AND {}", keyset.replace("?1", "?")));
                params.push(Box::new(c.seq));
            }
            let sql = format!(
                "SELECT {ROW_COLUMNS} FROM calls c LEFT JOIN tenant_keys k ON k.id = c.key_id \
                 WHERE {clause} {} LIMIT {limit}",
                CallsList::sql_order(CallsOrder::NewestFirst)
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map(params_from_iter(params.iter().map(|p| p.as_ref() as &dyn ToSql)), row_from)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }
}

/// `host.calls.list` (R2).
pub async fn list(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    use crate::paged::{self, CallsList, CallsOrder, OrderEnum, Paged};
    let fields = paged::parse_fields(CallsList::VERB, args)?;
    let rl = paged::resolve_limit(args, CallsList::DEFAULT_LIMIT.into(), CallsList::MAX_LIMIT.into())?;
    let order = paged::resolve_order::<CallsOrder>(args)?;
    let now = now_unix();
    let mut filter = filter_from_args(args, now)?;
    // R6: a sub-key sees only its own rows.
    filter.only_key_id = crate::subkeys::current_key_id();
    let cursor = match args.get("cursor") {
        Some(Value::String(raw)) => {
            let c = paged::Cursor::decode(raw, &state.session_bindings, tenant.id)?;
            c.require_order(order.name())?;
            Some(c)
        }
        _ => None,
    };
    let mut rows = state.db.list_call_rows(tenant.id, filter.clone(), cursor, rl.limit + 1).await?;
    let next = paged::split_page(&mut rows, rl.limit, |r| CallsList::cursor_at(r, order));
    let mut out = serde_json::Map::new();
    // R4: a window reaching past the plan's retention names the cutoff.
    if let Some(plan) = state.plans.get(&tenant.plan) {
        let cutoff = now - plan.calls_retention_days.saturating_mul(86_400);
        if filter.since_unix < cutoff {
            out.insert("truncated_before".to_string(), json!(rfc3339_from_unix(cutoff)));
        }
    }
    out.insert(CallsList::KEY.to_string(), json!(rows.iter().map(CallRow::to_json).collect::<Vec<_>>()));
    paged::finish_page(&mut out, next.as_ref(), &state.session_bindings, tenant.id);
    let mut value = Value::Object(out);
    if let Some(fields) = &fields {
        paged::project_page(&mut value, CallsList::KEY, fields);
    }
    paged::note_clamp(&mut value, &rl);
    Ok(value)
}

/// The most rows one export may hold (R3); a wider window is `too_many_rows`.
pub const EXPORT_ROWS_MAX: i64 = 50_000;

/// One RFC 4180 CSV field.
fn csv_field(out: &mut String, value: &str) {
    if value.contains([',', '"', '\n', '\r']) {
        out.push('"');
        out.push_str(&value.replace('"', "\"\""));
        out.push('"');
    } else {
        out.push_str(value);
    }
}

/// The export CSV: the [`CallRow::FIELDS`] header line, then one line per
/// row in the same column order (a null field is an empty cell).
pub fn render_csv(rows: &[CallRow]) -> String {
    let mut out = CallRow::FIELDS.join(",");
    out.push('\n');
    for row in rows {
        let json = row.to_json();
        for (i, field) in CallRow::FIELDS.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            match &json[*field] {
                Value::Null => {}
                Value::String(s) => csv_field(&mut out, s),
                other => csv_field(&mut out, &other.to_string()),
            }
        }
        out.push('\n');
    }
    out
}

/// ASN.1 DER `ECDSA-Sig-Value` from a fixed-width `r || s` signature, the
/// form `openssl dgst -verify` expects.
fn der_from_fixed(sig: &[u8]) -> Vec<u8> {
    fn der_int(bytes: &[u8]) -> Vec<u8> {
        let mut v: Vec<u8> = bytes.iter().copied().skip_while(|b| *b == 0).collect();
        if v.first().is_none_or(|b| b & 0x80 != 0) {
            v.insert(0, 0);
        }
        let mut out = vec![0x02, v.len() as u8];
        out.extend(v);
        out
    }
    let (r, s) = sig.split_at(sig.len() / 2);
    let mut body = der_int(r);
    body.extend(der_int(s));
    let mut out = vec![0x30, body.len() as u8];
    out.extend(body);
    out
}

/// The signed export of `tenant`'s rows in `filter`'s window (R3).
pub async fn build_bundle(state: &AppState, tenant: &Tenant, filter: CallFilter) -> Result<Value, AppError> {
    use base64::Engine as _;
    use sha2::{Digest, Sha256};
    // Count first: an over-cap window builds no CSV at all.
    let total = state.db.count_call_rows(tenant.id, filter.clone()).await?;
    if total > EXPORT_ROWS_MAX {
        return Err(AppError::Structured {
            code: "too_many_rows",
            message: format!(
                "the window holds {total} calls; an export is at most {EXPORT_ROWS_MAX} rows -- narrow since/until"
            ),
            data: json!({
                "rows": total,
                "max_rows": EXPORT_ROWS_MAX,
                "hint": "narrow the window: pass a later since or an earlier until, and export it in several parts",
            }),
        });
    }
    let rows = state.db.list_call_rows(tenant.id, filter, None, EXPORT_ROWS_MAX).await?;
    let csv = render_csv(&rows);
    let sha256: String = Sha256::digest(csv.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    let signature = state.authz_key.sign_bytes(csv.as_bytes())?;
    let fixed = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(&signature)
        .map_err(|e| AppError::Internal(format!("call export: bad signature encoding: {e}")))?;
    let signature_der = base64::engine::general_purpose::STANDARD.encode(der_from_fixed(&fixed));
    Ok(json!({
        "csv": csv,
        "rows": rows.len(),
        "sha256": sha256,
        "signature": signature,
        "signature_der": signature_der,
        "kid": state.authz_key.kid,
        "alg": "ES256",
    }))
}

/// `host.calls.export` (R3).
pub async fn export(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let filter = filter_from_args(args, now_unix())?;
    build_bundle(state, tenant, filter).await
}
