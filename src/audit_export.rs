//! `host.audit.export` / `host.audit.verify {root_hash}`: every audit row of a
//! tenant (`audit_chain`, `tenant_audit`, `oauth_audit`) as one hash-chained
//! JSONL with an ES256-signed root, verifiable offline by `scripts/audit-verify.py`.

use serde_json::{Map, Value, json};

use crate::errors::AppError;
use crate::rowpolicy::audit::{GENESIS_HASH, compute_export_line_hash};
use crate::state::{AppState, now_unix};
use crate::db::Tenant;

/// R2: the most rows one export may hold.
pub const EXPORT_ROWS_MAX: i64 = 50_000;

/// R6: the formats `host.audit.export` renders; the schema enum and the
/// argument check both read this.
pub const EXPORT_FORMATS: &[&str] = &["jsonl", "csv"];

/// The three source tables, in the order `(ts, table, id)` sorts them.
pub const TABLES: [&str; 3] = ["audit_chain", "oauth_audit", "tenant_audit"];

/// One audit row before it is chained.
#[derive(Debug, Clone)]
pub struct RawAuditRow {
    pub ts: i64,
    pub table: &'static str,
    pub id: i64,
    pub subject: String,
    pub action: String,
    pub detail: Value,
}

impl RawAuditRow {
    pub fn from_chain(r: crate::db::AuditRecordRow) -> Self {
        RawAuditRow {
            ts: r.timestamp_unix,
            table: "audit_chain",
            id: r.id,
            subject: r.subject,
            action: format!("filtered_read:{}", r.plane),
            detail: json!({
                "applied": r.applied,
                "policy_hash": r.policy_hash,
                "request_id": r.request_id,
                "returned_count": r.returned_count,
                "withheld_count": r.withheld_count,
                "stored_hash": r.record_hash,
                "prior_stored_hash": r.prior_record_hash,
            }),
        }
    }

    pub fn from_tenant(id: i64, subject: String, action: String, detail: Option<String>, ts: i64) -> Self {
        RawAuditRow { ts, table: "tenant_audit", id, subject, action, detail: detail.map_or(Value::Null, Value::String) }
    }

    pub fn from_oauth(r: crate::db::OauthAuditRow) -> Self {
        RawAuditRow {
            ts: r.ts,
            table: "oauth_audit",
            id: r.id,
            subject: r.end_user_subject.unwrap_or_default(),
            action: r.event,
            detail: json!({
                "client_id": r.client_id,
                "method": r.method,
                "reason": r.reason,
                "ip_hash": r.ip_hash,
                "scopes": r.scopes,
            }),
        }
    }
}

/// One `audit_exports` row: an export's root and the window it covered.
#[derive(Debug, Clone)]
pub struct StoredExport {
    pub tenant_id: i64,
    pub root_hash: String,
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub rows: i64,
    pub created_unix: i64,
}

/// One chained export line.
#[derive(Debug, Clone)]
pub struct AuditLine {
    pub row: RawAuditRow,
    pub prev: String,
    pub hash: String,
}

impl AuditLine {
    /// The line as the wire object (every field, `hash` last in the text only
    /// because the canonical form sorts keys).
    pub fn to_value(&self) -> Value {
        let mut m = self.body();
        m.insert("hash".to_string(), json!(self.hash));
        Value::Object(m)
    }

    fn body(&self) -> Map<String, Value> {
        body_of(&self.row, &self.prev)
    }
}

fn body_of(row: &RawAuditRow, prev: &str) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("ts".to_string(), json!(row.ts));
    m.insert("table".to_string(), json!(row.table));
    m.insert("id".to_string(), json!(row.id));
    m.insert("subject".to_string(), json!(row.subject));
    m.insert("action".to_string(), json!(row.action));
    m.insert("detail".to_string(), row.detail.clone());
    m.insert("prev".to_string(), json!(prev));
    m
}

/// The canonical form: compact JSON, object keys sorted at every depth, no
/// whitespace, non-ASCII left as UTF-8. Written out here (not left to
/// `serde_json`'s map type) so a feature flip cannot change it; the vector
/// file `tests/fixtures/audit_vectors.json` is its generated contract.
pub fn canonical_json(v: &Value) -> String {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .into_iter()
                .map(|k| format!("{}:{}", serde_json::to_string(k).expect("string"), canonical_json(&m[k])))
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        Value::Array(a) => format!("[{}]", a.iter().map(canonical_json).collect::<Vec<_>>().join(",")),
        other => serde_json::to_string(other).expect("scalar"),
    }
}

/// Chains `rows` (already in `(ts, table, id)` order) from the genesis, with a
/// pluggable canonical form so a test can prove the vector file pins it.
pub fn chain_with(rows: Vec<RawAuditRow>, canon: fn(&Value) -> String) -> Vec<AuditLine> {
    let mut prev = GENESIS_HASH.to_string();
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let body = Value::Object(body_of(&row, &prev));
        let hash = compute_export_line_hash(&prev, canon(&body).as_bytes());
        out.push(AuditLine { row, prev: prev.clone(), hash: hash.clone() });
        prev = hash;
    }
    out
}

pub fn chain(rows: Vec<RawAuditRow>) -> Vec<AuditLine> {
    chain_with(rows, canonical_json)
}

/// The root of a chained window: the last line's hash, or the genesis for none.
pub fn root_of(lines: &[AuditLine]) -> String {
    lines.last().map_or_else(|| GENESIS_HASH.to_string(), |l| l.hash.clone())
}

/// R2: what the signature covers -- `root_hash` then the decimal row count.
pub fn signed_message(root_hash: &str, rows: usize) -> String {
    format!("{root_hash}{rows}")
}

fn bound(args: &Value, field: &str, now: i64) -> Result<Option<i64>, AppError> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n
            .as_i64()
            .map(Some)
            .ok_or_else(|| AppError::InvalidArgs(format!("{field} must be a string or unix seconds"))),
        Some(Value::String(s)) => crate::calllog::parse_bound(s, now).map(Some).ok_or_else(|| {
            AppError::InvalidArgs(format!("{field} must be like \"-24h\", \"2026-10-01\", an RFC 3339 instant or unix seconds"))
        }),
        Some(_) => Err(AppError::InvalidArgs(format!("{field} must be a string or unix seconds"))),
    }
}

async fn window_lines(
    state: &AppState,
    tenant_id: i64,
    since: Option<i64>,
    until: Option<i64>,
    limit: i64,
) -> Result<Vec<AuditLine>, AppError> {
    let count = state.db.audit_export_count(tenant_id, since, until).await?;
    if count > limit {
        return Err(AppError::Structured {
            code: crate::export::EXPORT_TOO_LARGE,
            message: format!("{count} audit rows in this window, over the {limit} row export limit"),
            data: json!({"rows": count, "max_rows": limit, "hint": "narrow the window: pass a later since or an earlier until"}),
        });
    }
    Ok(chain(state.db.audit_export_rows(tenant_id, since, until).await?))
}

fn header_value(tenant: &Tenant, since: Option<i64>, until: Option<i64>, kid: &str) -> Value {
    json!({
        "tenant_ns": tenant.namespace,
        "since": since,
        "until": until,
        "host_version": env!("CARGO_PKG_VERSION"),
        "kid": kid,
    })
}

/// `host.audit.export {since?, until?, limit?, format?}` (R2, R6).
pub async fn export(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let now = now_unix();
    let since = bound(args, "since", now)?;
    let until = bound(args, "until", now)?;
    let limit = match args.get("limit") {
        None | Some(Value::Null) => EXPORT_ROWS_MAX,
        Some(v) => v
            .as_i64()
            .filter(|n| (1..=EXPORT_ROWS_MAX).contains(n))
            .ok_or_else(|| AppError::InvalidArgs(format!("limit must be an integer 1-{EXPORT_ROWS_MAX}")))?,
    };
    let format = match args.get("format").and_then(Value::as_str) {
        None => EXPORT_FORMATS[0],
        Some(f) => EXPORT_FORMATS
            .iter()
            .copied()
            .find(|known| *known == f)
            .ok_or_else(|| AppError::InvalidArgs(format!("format must be one of: {}", EXPORT_FORMATS.join(", "))))?,
    };
    let lines = window_lines(state, tenant.id, since, until, limit).await?;
    let root_hash = root_of(&lines);
    let rows = lines.len();
    let kid = state.authz_key.kid.clone();
    let signature = state.authz_key.sign_bytes(signed_message(&root_hash, rows).as_bytes())?;
    state
        .db
        .audit_export_store(StoredExport {
            tenant_id: tenant.id,
            root_hash: root_hash.clone(),
            since,
            until,
            rows: rows as i64,
            created_unix: now,
        })
        .await?;
    let header = header_value(tenant, since, until, &kid);
    let trailer = json!({"root_hash": root_hash, "rows": rows, "signature": signature, "kid": kid});
    let mut out = json!({
        "format": format,
        "rows": rows,
        "root_hash": root_hash,
        "signature": signature,
        "kid": kid,
        "alg": "ES256",
    });
    if format == EXPORT_FORMATS[0] {
        let mut text = String::new();
        for v in std::iter::once(&header).chain(lines.iter().map(AuditLine::to_value).collect::<Vec<_>>().iter()).chain(std::iter::once(&trailer)) {
            text.push_str(&canonical_json(v));
            text.push('\n');
        }
        out["jsonl"] = json!(text);
    } else {
        out["csv"] = json!(render_csv(&lines, &root_hash, rows, &signature, &kid));
    }
    Ok(out)
}

/// R6: the same lines as CSV; the header row names the columns and the last
/// row is the trailer (`table = trailer`, `id` = row count).
pub fn render_csv(lines: &[AuditLine], root_hash: &str, rows: usize, signature: &str, kid: &str) -> String {
    use crate::calllog::csv_field;
    const COLUMNS: [&str; 11] =
        ["ts", "table", "id", "subject", "action", "detail", "prev", "hash", "root_hash", "signature", "kid"];
    let mut out = COLUMNS.join(",");
    out.push('\n');
    let mut push = |cells: [String; 11]| {
        for (i, c) in cells.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            csv_field(&mut out, c);
        }
        out.push('\n');
    };
    for l in lines {
        push([
            l.row.ts.to_string(),
            l.row.table.to_string(),
            l.row.id.to_string(),
            l.row.subject.clone(),
            l.row.action.clone(),
            canonical_json(&l.row.detail),
            l.prev.clone(),
            l.hash.clone(),
            String::new(),
            String::new(),
            String::new(),
        ]);
    }
    push([
        String::new(),
        "trailer".to_string(),
        rows.to_string(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        root_hash.to_string(),
        signature.to_string(),
        kid.to_string(),
    ]);
    out
}

/// `host.audit.verify {root_hash}` (R3): recompute the chain over the window
/// stored with the export and compare.
pub async fn verify_root(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let root = args
        .get("root_hash")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::InvalidArgs("'root_hash' must be a string".to_string()))?;
    let Some((since, until, rows, created)) = state.db.audit_export_get(tenant.id, root.to_string()).await? else {
        return Err(AppError::Structured {
            code: crate::subkeys::NOT_FOUND,
            message: format!("no export of this tenant has root_hash {root}"),
            data: json!({"root_hash": root}),
        });
    };
    // Rows stamped after the export was made are not part of what it signed.
    let through = until.map_or(created + 1, |u| u.min(created + 1));
    let lines = window_lines(state, tenant.id, since, Some(through), i64::MAX).await?;
    let verified = root_of(&lines) == root && lines.len() as i64 == rows;
    Ok(json!({"verified": verified, "rows": lines.len(), "through_unix": through - 1}))
}
