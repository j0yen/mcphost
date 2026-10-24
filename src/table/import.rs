//! PRD-mcphost-table-import-from-url: `source_url` (an https CSV or JSON Lines
//! file) and `csv_text` (inline, at most 1 MB) on `host.table.create` and
//! `host.table.append`, so a client with no file tool can load a table.
//!
//! One pipeline for both verbs: take the single source argument
//! ([`source_arg`]), fetch it through the SSRF guard ([`crate::net::fetch_guard`])
//! or read the inline text, parse it ([`parse`]), then write it in one
//! transaction ([`write`]). `create` infers column types from the first
//! [`INFER_ROWS`] rows and creates the table first; `append` coerces each row to
//! the table's own schema and skips the rows that fail, keeping the first
//! [`SKIP_REASONS_MAX`] reasons.
//!
//! Every import (and every refused one) leaves a row in the tenant's own
//! `_mcphost_imports` table: tool, source host, bytes received, rows, outcome.
//! That row is the audit ledger, the fetch counter a refusal is proved against
//! (a refused import records 0 bytes) and the `import_id` replay store.

use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Map, Value, json};

use crate::db::Tenant;
use crate::errors::AppError;
use crate::net::fetch_guard::{self, FetchError, FetchPolicy};
use crate::state::AppState;
use crate::tables::{self, ColumnType};

/// The arguments that each name a whole data source; exactly one may be given.
pub const SOURCE_ARGS: &[&str] = &["rows", "source_url", "csv_text"];
/// Inline `csv_text` ceiling.
pub const INLINE_MAX_BYTES: u64 = 1024 * 1024;
/// Rows the column types of a new table are inferred from.
pub const INFER_ROWS: usize = 1000;
/// Skip reasons a response carries.
pub const SKIP_REASONS_MAX: usize = 3;
/// How long an `import_id` replays its first result.
pub const REPLAY_WINDOW_S: i64 = 24 * 60 * 60;
const IMPORT_ID_MAX: usize = 128;

crate::err_code! {
    pub IMPORT_URL_FORBIDDEN,
    code: "import_url_forbidden", family: Upstream, class: HostRefusal, retryable: false,
    remedy: "Use a public https URL; data.host names the host the guard refused and data.reason why.",
    data_keys: &["host", "reason"],
}

crate::err_code! {
    pub IMPORT_FETCH_FAILED,
    code: "import_fetch_failed", family: Upstream, class: UpstreamFault, retryable: true,
    remedy: "Check the URL is public and returns CSV or JSON Lines; data.hint names the fix when one is known.",
    data_keys: &["host", "status", "hint"],
}

crate::err_code! {
    pub IMPORT_TOO_LARGE,
    code: "import_too_large", family: Plan, class: PlanLimit, retryable: false,
    remedy: "Send a smaller file or split it; data.bytes is the byte cap that applied.",
    data_keys: &["bytes", "host"],
}

crate::err_code! {
    pub IMPORT_PARSE_FAILED,
    code: "import_parse_failed", family: Args, class: AgentFault, retryable: false,
    remedy: "Fix the source at data.line (RFC 4180 CSV with a header row, or one JSON object per line) and retry.",
    data_keys: &["line", "column"],
}

crate::err_code! {
    pub IMPORT_SCHEMA_MISMATCH,
    code: "import_schema_mismatch", family: Storage, class: AgentFault, retryable: false,
    remedy: "Rename the source's columns to the table's (data.expected) or add the missing columns; data.skip_reasons has the first failures.",
    data_keys: &["expected", "unknown_columns", "rows_skipped", "skip_reasons"],
}

/// Where the rows come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Url(String),
    Text(String),
}

/// The import source `args` names, `None` for the plain `rows` / `columns`
/// path. More than one of [`SOURCE_ARGS`] is refused here too, behind the
/// inputSchema `oneOf` that refuses it first.
pub fn source_arg(args: &Value) -> Result<Option<Source>, AppError> {
    let present: Vec<&str> = SOURCE_ARGS
        .iter()
        .copied()
        .filter(|k| args.get(*k).is_some_and(|v| !v.is_null()))
        .collect();
    if present.len() > 1 {
        return Err(AppError::InvalidArgs(format!(
            "supply exactly one of rows, source_url or csv_text; got {}",
            present.join(" and ")
        )));
    }
    let text = |key: &str| {
        args.get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| AppError::InvalidArgs(format!("'{key}' must be a string")))
    };
    match present.first().copied() {
        Some("source_url") => Ok(Some(Source::Url(text("source_url")?))),
        Some("csv_text") => Ok(Some(Source::Text(text("csv_text")?))),
        _ => Ok(None),
    }
}

fn import_id_arg(args: &Value) -> Result<Option<String>, AppError> {
    match args.get("import_id") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if !s.is_empty() && s.len() <= IMPORT_ID_MAX => Ok(Some(s.clone())),
        Some(_) => Err(AppError::InvalidArgs(format!("import_id: must be a non-empty string of at most {IMPORT_ID_MAX} bytes"))),
    }
}

fn structured(code: &'static str, message: String, data: Value) -> AppError {
    AppError::Structured { code, message, data }
}

fn too_large(cap: u64, host: Option<&str>) -> AppError {
    structured(
        IMPORT_TOO_LARGE,
        format!("the source is larger than the {cap}-byte import cap"),
        json!({"bytes": cap, "host": host}),
    )
}

fn fetch_error(e: FetchError) -> AppError {
    match e {
        FetchError::Forbidden { host, reason } => structured(
            IMPORT_URL_FORBIDDEN,
            format!("source_url host '{host}' is not allowed: {reason}"),
            json!({"host": host, "reason": reason}),
        ),
        FetchError::Failed { host, detail, status, hint } => {
            let mut data = json!({"host": host, "status": status});
            if let Some(hint) = hint {
                data["hint"] = json!(hint);
            }
            structured(IMPORT_FETCH_FAILED, format!("fetching from '{host}' failed: {detail}"), data)
        }
        FetchError::TooLarge { host, cap } => too_large(cap, Some(&host)),
    }
}

fn parse_failed(line: usize, column: Option<&str>, why: impl std::fmt::Display) -> AppError {
    structured(
        IMPORT_PARSE_FAILED,
        format!("line {line}: {why}"),
        json!({"line": line, "column": column}),
    )
}

// ---- parsing -----------------------------------------------------------------

/// One data row: its 1-based line (CSV: where the record starts) and its
/// non-null cells in source order.
#[derive(Debug, Clone)]
pub struct ParsedRow {
    pub line: usize,
    pub cells: Vec<(String, Value)>,
}

/// Rows an import dropped: how many, and the reasons of the first
/// [`SKIP_REASONS_MAX`] by line.
#[derive(Debug, Clone, Default)]
pub struct Skips {
    pub count: usize,
    first: Vec<(usize, String)>,
}

impl Skips {
    /// Records one skipped row. Lines arrive ascending within one source of skips.
    fn push(&mut self, line: usize, reason: String) {
        self.count += 1;
        if self.first.len() < SKIP_REASONS_MAX {
            self.first.push((line, reason));
        }
    }

    /// Both lists' skips, the first [`SKIP_REASONS_MAX`] reasons in line order.
    fn merged(&self, other: &Skips) -> Skips {
        let mut first: Vec<(usize, String)> = self.first.iter().chain(&other.first).cloned().collect();
        first.sort_by_key(|(line, _)| *line);
        first.truncate(SKIP_REASONS_MAX);
        Skips { count: self.count + other.count, first }
    }

    /// The reasons, in line order.
    pub fn reasons(&self) -> Vec<String> {
        self.first.iter().map(|(_, r)| r.clone()).collect()
    }
}

/// A parsed source.
#[derive(Debug, Clone)]
pub struct Parsed {
    /// Column names (normalized to table identifiers), in source order.
    pub header: Vec<String>,
    pub rows: Vec<ParsedRow>,
    /// Rows the parser itself dropped (wrong field count, not JSON).
    pub skips: Skips,
}

/// A source column name as a table identifier: runs of characters outside
/// `[A-Za-z0-9_]` become one `_`, a leading digit gains a `_`, at most 64 bytes.
pub fn normalize_ident(raw: &str, position: usize) -> String {
    let mut out = String::new();
    let mut gap = false;
    for c in raw.trim().chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            if gap && !out.is_empty() {
                out.push('_');
            }
            gap = false;
            out.push(c);
        } else {
            gap = true;
        }
    }
    if out.is_empty() {
        return format!("col{position}");
    }
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out.truncate(64);
    out
}

fn dedupe_idents(names: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    names
        .into_iter()
        .map(|n| {
            let mut candidate = n.clone();
            let mut k = 2;
            while !seen.insert(candidate.to_ascii_lowercase()) {
                candidate = format!("{n}_{k}");
                k += 1;
            }
            candidate
        })
        .collect()
}

/// One CSV record: the line it starts on and its fields.
type CsvRecord = (usize, Vec<String>);

/// RFC 4180 records of `text`: `(line the record starts on, fields)`. Quoted
/// fields may hold the delimiter, doubled quotes and line breaks; a quote in
/// the middle of an unquoted field is literal. Blank lines are not records.
fn csv_records(text: &str) -> Result<Vec<CsvRecord>, AppError> {
    let mut records = Vec::new();
    let mut fields: Vec<String> = Vec::new();
    let mut field = String::new();
    let (mut line, mut start) = (1usize, 1usize);
    let mut in_quotes = false;
    let mut field_was_quoted = false;
    let mut chars = text.chars().peekable();
    let finish = |fields: &mut Vec<String>, field: &mut String, start: usize, records: &mut Vec<CsvRecord>| {
        fields.push(std::mem::take(field));
        let blank = fields.len() == 1 && fields[0].is_empty();
        let rec = std::mem::take(fields);
        if !blank {
            records.push((start, rec));
        }
    };
    while let Some(c) = chars.next() {
        if in_quotes {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' => in_quotes = false,
                '\n' => {
                    line += 1;
                    field.push('\n');
                }
                _ => field.push(c),
            }
            continue;
        }
        match c {
            '"' if field.is_empty() && !field_was_quoted => {
                in_quotes = true;
                field_was_quoted = true;
            }
            ',' => {
                fields.push(std::mem::take(&mut field));
                field_was_quoted = false;
            }
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' => {
                finish(&mut fields, &mut field, start, &mut records);
                field_was_quoted = false;
                line += 1;
                start = line;
            }
            _ => field.push(c),
        }
    }
    if in_quotes {
        return Err(parse_failed(start, None, "a quoted field is never closed"));
    }
    if !field.is_empty() || !fields.is_empty() {
        finish(&mut fields, &mut field, start, &mut records);
    }
    Ok(records)
}

fn parse_csv(text: &str) -> Result<Parsed, AppError> {
    let mut records = csv_records(text)?.into_iter();
    let Some((_, head)) = records.next() else {
        return Err(parse_failed(1, None, "the source has no header row"));
    };
    let header = dedupe_idents(head.iter().enumerate().map(|(i, h)| normalize_ident(h, i + 1)).collect());
    let mut parsed = Parsed { header, rows: Vec::new(), skips: Skips::default() };
    for (line, fields) in records {
        if fields.len() != parsed.header.len() {
            parsed.skips.push(line, format!("line {line}: expected {} fields, got {}", parsed.header.len(), fields.len()));
            continue;
        }
        let cells: Vec<(String, Value)> = parsed
            .header
            .iter()
            .zip(fields)
            .filter(|(_, v)| !v.is_empty())
            .map(|(h, v)| (h.clone(), Value::String(v)))
            .collect();
        if cells.is_empty() {
            parsed.skips.push(line, format!("line {line}: every field is empty"));
            continue;
        }
        parsed.rows.push(ParsedRow { line, cells });
    }
    Ok(parsed)
}

fn parse_jsonl(text: &str) -> Result<Parsed, AppError> {
    let mut parsed = Parsed { header: Vec::new(), rows: Vec::new(), skips: Skips::default() };
    let trimmed = text.trim_start();
    let objects: Vec<(usize, Result<Value, String>)> = if trimmed.starts_with('[') {
        match serde_json::from_str::<Vec<Value>>(trimmed) {
            Ok(items) => items.into_iter().enumerate().map(|(i, v)| (i + 1, Ok(v))).collect(),
            Err(e) => return Err(parse_failed(e.line(), None, format!("not a JSON array of objects: {e}"))),
        }
    } else {
        text.lines()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty())
            .map(|(i, l)| (i + 1, serde_json::from_str::<Value>(l).map_err(|e| e.to_string())))
            .collect()
    };
    for (line, item) in objects {
        let obj = match item {
            Ok(Value::Object(o)) => o,
            Ok(_) => {
                parsed.skips.push(line, format!("line {line}: expected a JSON object"));
                continue;
            }
            Err(e) => {
                parsed.skips.push(line, format!("line {line}: not valid JSON ({e})"));
                continue;
            }
        };
        let mut cells: Vec<(String, Value)> = Vec::with_capacity(obj.len());
        for (i, (key, value)) in obj.into_iter().enumerate() {
            let name = normalize_ident(&key, i + 1);
            if !parsed.header.contains(&name) {
                parsed.header.push(name.clone());
            }
            if !value.is_null() && !cells.iter().any(|(c, _)| *c == name) {
                cells.push((name, value));
            }
        }
        if cells.is_empty() {
            parsed.skips.push(line, format!("line {line}: the object has no values"));
            continue;
        }
        parsed.rows.push(ParsedRow { line, cells });
    }
    Ok(parsed)
}

/// JSON Lines when the `Content-Type` says json or the text starts with `{` / `[`; else CSV.
fn is_jsonl(content_type: Option<&str>, text: &str) -> bool {
    let ct = content_type.unwrap_or("").to_ascii_lowercase();
    if ct.contains("json") {
        return true;
    }
    if ct.contains("csv") {
        return false;
    }
    text.trim_start().starts_with(['{', '['])
}

/// Parses `bytes` (UTF-8, a leading BOM allowed) as CSV or JSON Lines.
pub fn parse(bytes: &[u8], content_type: Option<&str>) -> Result<Parsed, AppError> {
    let text = std::str::from_utf8(bytes).map_err(|e| {
        let line = bytes[..e.valid_up_to()].iter().filter(|b| **b == b'\n').count() + 1;
        parse_failed(line, None, "the source is not valid UTF-8")
    })?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let parsed = if is_jsonl(content_type, text) { parse_jsonl(text)? } else { parse_csv(text)? };
    if parsed.rows.is_empty() {
        return Err(parse_failed(1, None, "the source holds no data rows"));
    }
    Ok(parsed)
}

// ---- types -------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Seen {
    Nothing,
    Bool,
    Int,
    Float,
    Text,
    Json,
}

fn seen_of(value: &Value) -> Seen {
    match value {
        Value::Bool(_) => Seen::Bool,
        Value::Number(n) if n.is_i64() => Seen::Int,
        Value::Number(_) => Seen::Float,
        Value::Array(_) | Value::Object(_) => Seen::Json,
        Value::String(s) => {
            let s = Value::String(s.trim().to_string());
            let text = s.as_str().unwrap_or_default();
            if text.eq_ignore_ascii_case("true") || text.eq_ignore_ascii_case("false") {
                Seen::Bool
            } else if tables::coerce_canonical_numeral("integer", &s).is_some_and(|v| v.is_i64()) {
                Seen::Int
            } else if tables::coerce_canonical_numeral("real", &s).is_some() {
                Seen::Float
            } else {
                Seen::Text
            }
        }
        Value::Null => Seen::Nothing,
    }
}

fn widen(a: Seen, b: Seen) -> Seen {
    match (a, b) {
        (x, y) if x == y => x,
        (Seen::Nothing, y) => y,
        (x, Seen::Nothing) => x,
        (Seen::Int, Seen::Float) | (Seen::Float, Seen::Int) => Seen::Float,
        _ => Seen::Text,
    }
}

/// Column types for a new table, from the first [`INFER_ROWS`] rows: bool, int,
/// float, json, else text. A column with no value in them is text.
pub fn infer_columns(parsed: &Parsed) -> Vec<(String, ColumnType)> {
    let mut seen = vec![Seen::Nothing; parsed.header.len()];
    for row in parsed.rows.iter().take(INFER_ROWS) {
        for (name, value) in &row.cells {
            if let Some(i) = parsed.header.iter().position(|h| h == name) {
                seen[i] = widen(seen[i], seen_of(value));
            }
        }
    }
    parsed
        .header
        .iter()
        .zip(seen)
        .map(|(h, s)| {
            let ty = match s {
                Seen::Bool => ColumnType::Boolean,
                Seen::Int => ColumnType::Integer,
                Seen::Float => ColumnType::Real,
                Seen::Json => ColumnType::Json,
                Seen::Text | Seen::Nothing => ColumnType::Text,
            };
            (h.clone(), ty)
        })
        .collect()
}

fn snippet(value: &Value) -> String {
    let text = match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    let short: String = text.chars().take(40).collect();
    if short.len() < text.len() { format!("{short}...") } else { short }
}

/// `value` as a `ty` cell, or why it is not one.
fn coerce_cell(ty: ColumnType, value: &Value) -> Result<Value, String> {
    let fail = || format!("expected {}, got '{}'", ty.as_str(), snippet(value));
    if ty != ColumnType::Text && ty != ColumnType::Json && ty.matches(value) {
        return Ok(value.clone());
    }
    let trimmed = || Value::String(value.as_str().unwrap_or_default().trim().to_string());
    match ty {
        ColumnType::Text | ColumnType::Timestamp => match value {
            Value::String(_) => Ok(value.clone()),
            Value::Number(_) | Value::Bool(_) if ty == ColumnType::Text => Ok(Value::String(value.to_string())),
            _ => Err(fail()),
        },
        ColumnType::Integer => tables::coerce_canonical_numeral("integer", &trimmed()).ok_or_else(fail),
        ColumnType::Real => tables::coerce_canonical_numeral("real", &trimmed()).ok_or_else(fail),
        ColumnType::Boolean => {
            match trimmed().as_str().map(str::to_ascii_lowercase).as_deref() {
                Some("true") => Ok(Value::Bool(true)),
                Some("false") => Ok(Value::Bool(false)),
                _ => Err(fail()),
            }
        }
        ColumnType::Json => match value {
            Value::String(s) => serde_json::from_str(s).map_err(|_| fail()),
            other => Ok(other.clone()),
        },
    }
}

/// Rows ready to insert, plus what was skipped (parser and converter together).
pub struct Converted {
    pub rows: Vec<Map<String, Value>>,
    pub skips: Skips,
}

/// Coerces every parsed row to `schema`; a row with an unknown column or a cell
/// that is not its column's type is skipped with a reason naming line and column.
pub fn convert_rows(parsed: &Parsed, schema: &std::collections::BTreeMap<String, ColumnType>) -> Converted {
    let mut rows = Vec::with_capacity(parsed.rows.len());
    let mut skips = Skips::default();
    'rows: for row in &parsed.rows {
        let mut cells = Map::with_capacity(row.cells.len());
        for (name, value) in &row.cells {
            let Some(ty) = schema.get(name) else {
                skips.push(row.line, format!("line {}, column \"{name}\": not a column of the table", row.line));
                continue 'rows;
            };
            match coerce_cell(*ty, value) {
                Ok(v) => {
                    cells.insert(name.clone(), v);
                }
                Err(why) => {
                    skips.push(row.line, format!("line {}, column \"{name}\": {why}", row.line));
                    continue 'rows;
                }
            }
        }
        rows.push(cells);
    }
    Converted { rows, skips: parsed.skips.merged(&skips) }
}

// ---- ledger ------------------------------------------------------------------

const LEDGER_DDL: &str = "CREATE TABLE IF NOT EXISTS _mcphost_imports (\
    id INTEGER PRIMARY KEY, import_id TEXT, tool TEXT NOT NULL, table_name TEXT NOT NULL, \
    source TEXT NOT NULL, source_host TEXT, bytes INTEGER NOT NULL, rows_imported INTEGER NOT NULL, \
    rows_skipped INTEGER NOT NULL, outcome TEXT NOT NULL, result_json TEXT, created_unix INTEGER NOT NULL); \
    CREATE INDEX IF NOT EXISTS _mcphost_imports_id ON _mcphost_imports (import_id)";

/// One import attempt as the ledger records it.
#[derive(Clone)]
struct Attempt {
    import_id: Option<String>,
    tool: &'static str,
    table: String,
    source: &'static str,
    source_host: Option<String>,
    bytes: u64,
    rows_imported: i64,
    rows_skipped: i64,
    outcome: String,
    result: Option<Value>,
}

fn ledger_insert(conn: &Connection, a: &Attempt) -> Result<(), AppError> {
    conn.execute_batch(LEDGER_DDL)?;
    conn.execute(
        "INSERT INTO _mcphost_imports (import_id, tool, table_name, source, source_host, bytes, rows_imported, \
         rows_skipped, outcome, result_json, created_unix) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            a.import_id,
            a.tool,
            a.table,
            a.source,
            a.source_host,
            a.bytes as i64,
            a.rows_imported,
            a.rows_skipped,
            a.outcome,
            a.result.as_ref().map(Value::to_string),
            crate::state::now_unix()
        ],
    )?;
    Ok(())
}

fn replay_sync(conn: &Connection, tool: &str, table: &str, import_id: &str) -> Result<Option<Value>, AppError> {
    conn.execute_batch(LEDGER_DDL)?;
    let stored: Option<String> = conn
        .query_row(
            "SELECT result_json FROM _mcphost_imports WHERE import_id = ?1 AND tool = ?2 AND table_name = ?3 \
             AND outcome = 'ok' AND created_unix > ?4 ORDER BY id LIMIT 1",
            params![import_id, tool, table, crate::state::now_unix() - REPLAY_WINDOW_S],
            |r| r.get(0),
        )
        .optional()?;
    Ok(stored.and_then(|s| serde_json::from_str(&s).ok()))
}

async fn replay(state: &AppState, tenant: &Tenant, tool: &'static str, table: &str, import_id: &str) -> Result<Option<Value>, AppError> {
    let (table, id) = (table.to_string(), import_id.to_string());
    let path = tables::tenant_db_path(state, tenant.id);
    tables::with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| replay_sync(conn, tool, &table, &id)).await
}

async fn record(state: &AppState, tenant: &Tenant, attempt: Attempt) {
    let path = tables::tenant_db_path(state, tenant.id);
    let res = tables::with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| ledger_insert(conn, &attempt)).await;
    if let Err(e) = res {
        tracing::warn!(error = %e, "failed to record a table import in the ledger");
    }
}

/// The tenant's import ledger, oldest first: one object per attempt.
pub async fn ledger(state: &AppState, tenant: &Tenant) -> Result<Vec<Value>, AppError> {
    let path = tables::tenant_db_path(state, tenant.id);
    tables::with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
        conn.execute_batch(LEDGER_DDL)?;
        let mut stmt = conn.prepare(
            "SELECT import_id, tool, table_name, source, source_host, bytes, rows_imported, rows_skipped, outcome, created_unix \
             FROM _mcphost_imports ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(json!({
                "import_id": r.get::<_, Option<String>>(0)?, "tool": r.get::<_, String>(1)?, "table": r.get::<_, String>(2)?,
                "source": r.get::<_, String>(3)?, "source_host": r.get::<_, Option<String>>(4)?, "bytes": r.get::<_, i64>(5)?,
                "rows_imported": r.get::<_, i64>(6)?, "rows_skipped": r.get::<_, i64>(7)?, "outcome": r.get::<_, String>(8)?,
                "created_unix": r.get::<_, i64>(9)?,
            }))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(AppError::from)
    })
    .await
}

// ---- load and write ----------------------------------------------------------

/// Where the bytes came from, for the response and the ledger.
#[derive(Clone)]
struct Origin {
    /// `"url"` or `"inline"`.
    source: &'static str,
    host: Option<String>,
    /// Body bytes received (inline: the text's length).
    bytes: u64,
}

/// One import call's identity: which verb, which table, which retry key.
struct Target<'a> {
    tool: &'static str,
    table: &'a str,
    import_id: Option<&'a str>,
}

impl Target<'_> {
    /// The ledger row for this call, `outcome` and counts to be filled in.
    fn attempt(&self, origin: &Origin) -> Attempt {
        Attempt {
            import_id: self.import_id.map(str::to_string),
            tool: self.tool,
            table: self.table.to_string(),
            source: origin.source,
            source_host: origin.host.clone(),
            bytes: origin.bytes,
            rows_imported: 0,
            rows_skipped: 0,
            outcome: String::new(),
            result: None,
        }
    }
}

/// A source fetched and parsed, not yet written.
struct Loaded {
    parsed: Parsed,
    origin: Origin,
}

/// The byte cap for `tenant`'s imports: the plan's table byte quota, at most [`fetch_guard::DEFAULT_CAP_BYTES`].
fn cap_bytes(state: &AppState, tenant: &Tenant) -> Result<u64, AppError> {
    let plan = tables::plan_of(state, &tenant.plan)?;
    Ok(fetch_guard::DEFAULT_CAP_BYTES.min(plan.table_bytes_max.max(0) as u64))
}

fn fetch_policy(state: &AppState) -> FetchPolicy {
    FetchPolicy { loopback_dev: crate::index::url_policy(state).1 }
}

/// A failed load: the error and what had been received when it failed.
type LoadFailure = Box<(AppError, Origin)>;

async fn load(state: &AppState, tenant: &Tenant, source: &Source) -> Result<Loaded, LoadFailure> {
    let mut origin = Origin { source: "inline", host: None, bytes: 0 };
    let (bytes, content_type) = match source {
        Source::Text(text) => {
            origin.bytes = text.len() as u64;
            if origin.bytes > INLINE_MAX_BYTES {
                return Err(Box::new((too_large(INLINE_MAX_BYTES, None), origin)));
            }
            (text.as_bytes().to_vec(), None)
        }
        Source::Url(url) => {
            origin.source = "url";
            let cap = cap_bytes(state, tenant).map_err(|e| Box::new((e, origin.clone())))?;
            if let Ok(parsed_url) = reqwest::Url::parse(url) {
                origin.host = parsed_url.host_str().map(str::to_string);
            }
            let received = AtomicU64::new(0);
            let fetched = fetch_guard::fetch_capped(url, cap, fetch_policy(state), None, &received).await;
            origin.bytes = received.load(Ordering::SeqCst);
            match fetched {
                Ok(f) => (f.bytes, f.content_type),
                Err(e) => return Err(Box::new((fetch_error(e), origin))),
            }
        }
    };
    match parse(&bytes, content_type.as_deref()) {
        Ok(parsed) => Ok(Loaded { parsed, origin }),
        Err(e) => Err(Box::new((e, origin))),
    }
}

/// What a write produced.
struct Written {
    imported: usize,
    skips: Skips,
    columns: Vec<(String, ColumnType)>,
}

/// How [`write`] ended.
enum Wrote {
    Rows(Written),
    /// A racing call with the same `import_id` finished first: its result.
    Replayed(Value),
}

fn columns_json(columns: &[(String, ColumnType)]) -> Value {
    Value::Array(columns.iter().map(|(n, t)| json!({"name": n, "type": t.as_str()})).collect())
}

/// Inserts the parsed rows into `target.table` in one transaction, honouring the
/// plan's row and byte quotas, and records the ledger row beside them.
async fn write(state: &AppState, tenant: &Tenant, target: &Target<'_>, loaded: &Loaded) -> Result<Wrote, AppError> {
    let plan = tables::plan_of(state, &tenant.plan)?;
    let (rows_max, bytes_max) = (plan.table_rows_max, plan.table_bytes_max);
    let (catalog, billing_mode, plan_name) = (state.plans.clone(), state.billing_config.billing_mode(), tenant.plan.clone());
    let path = tables::tenant_db_path(state, tenant.id);
    // The closure must be 'static: hand it owned copies.
    let (tool, table_name, import_id) = (target.tool, target.table.to_string(), target.import_id.map(str::to_string));
    let owned = loaded.parsed.clone();
    let origin = loaded.origin.clone();
    let base = target.attempt(&origin);
    tables::with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
        conn.execute_batch(LEDGER_DDL)?;
        if let Some(id) = &import_id
            && let Some(first) = replay_sync(conn, tool, &table_name, id)?
        {
            return Ok(Wrote::Replayed(first));
        }
        let schema = tables::load_schema_sync(conn, &table_name)?;
        let attempt = |outcome: &str, imported: i64, skipped: i64, result: Option<Value>| Attempt {
            outcome: outcome.to_string(),
            rows_imported: imported,
            rows_skipped: skipped,
            result,
            ..base.clone()
        };
        if owned.header.iter().all(|h| !schema.columns.contains_key(h)) {
            let expected: Vec<&String> = schema.columns.keys().collect();
            ledger_insert(conn, &attempt(IMPORT_SCHEMA_MISMATCH, 0, owned.rows.len() as i64, None))?;
            return Err(structured(
                IMPORT_SCHEMA_MISMATCH,
                format!(
                    "none of the source's columns ({}) is a column of '{table_name}' ({})",
                    owned.header.join(", "),
                    expected.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
                ),
                json!({"expected": expected, "unknown_columns": owned.header, "rows_skipped": owned.rows.len(), "skip_reasons": []}),
            ));
        }
        let converted = convert_rows(&owned, &schema.columns);
        if converted.rows.is_empty() {
            ledger_insert(conn, &attempt(IMPORT_SCHEMA_MISMATCH, 0, converted.skips.count as i64, None))?;
            return Err(structured(
                IMPORT_SCHEMA_MISMATCH,
                format!("no row fits '{table_name}': {}", converted.skips.reasons().first().cloned().unwrap_or_default()),
                json!({"expected": schema.columns.keys().collect::<Vec<_>>(), "unknown_columns": [], "rows_skipped": converted.skips.count, "skip_reasons": converted.skips.reasons()}),
            ));
        }
        let existing = tables::row_count_sync(conn, &table_name)?;
        if existing + converted.rows.len() as i64 > rows_max {
            return Err(crate::billing::quota_exceeded_for(&catalog, billing_mode, &plan_name, "table_rows_max", rows_max, existing, None));
        }
        let added: i64 = converted.rows.iter().map(|r| serde_json::to_string(r).map(|s| s.len() as i64).unwrap_or(0)).sum();
        let file_bytes = conn.path().and_then(|p| std::fs::metadata(p).ok()).map(|m| m.len() as i64).unwrap_or(0);
        if file_bytes + added > bytes_max {
            return Err(crate::billing::quota_exceeded_for(&catalog, billing_mode, &plan_name, "table_bytes_max", bytes_max, file_bytes, None));
        }
        tables::insert_rows_sync(conn, &table_name, &schema, &converted.rows)?;
        let columns: Vec<(String, ColumnType)> = owned
            .header
            .iter()
            .filter_map(|h| schema.columns.get(h).map(|t| (h.clone(), *t)))
            .collect();
        let written = Written { imported: converted.rows.len(), skips: converted.skips, columns };
        // The ledger row carries the result an `import_id` replay returns.
        let target = Target { tool, table: &table_name, import_id: import_id.as_deref() };
        let result = result_json(&target, &written, &origin);
        ledger_insert(conn, &attempt("ok", written.imported as i64, written.skips.count as i64, Some(result)))?;
        Ok(Wrote::Rows(written))
    })
    .await
}

fn result_json(target: &Target<'_>, w: &Written, origin: &Origin) -> Value {
    let mut out = if target.tool == "host.table.create" {
        json!({"name": target.table, "created": true})
    } else {
        json!({"table": target.table, "appended": w.imported, "ids": []})
    };
    out["rows_imported"] = json!(w.imported);
    out["rows_skipped"] = json!(w.skips.count);
    out["skip_reasons"] = json!(w.skips.reasons());
    out["columns"] = columns_json(&w.columns);
    out["source"] = json!(origin.source);
    if let Some(h) = &origin.host {
        out["source_host"] = json!(h);
    }
    out["bytes"] = json!(origin.bytes);
    out
}

async fn after_write(state: &AppState, tenant: &Tenant, table: &str, rewrite: bool) {
    if let Err(e) = state.db.mark_table_model_stale(tenant.id, table.to_string()).await {
        tracing::warn!(error = %e, table = %table, "failed to mark table model stale after import");
    }
    if !rewrite && let Err(e) = state.db.mark_table_graph_stale(tenant.id).await {
        tracing::warn!(error = %e, table = %table, "failed to mark table graph stale after import");
    }
    tables::bump_change_counter(state, tenant.id, table, rewrite).await;
}

/// Records a failed import in the ledger and hands the error back.
async fn fail(state: &AppState, tenant: &Tenant, target: &Target<'_>, origin: &Origin, err: AppError) -> AppError {
    let attempt = Attempt { outcome: err.code().to_string(), ..target.attempt(origin) };
    record(state, tenant, attempt).await;
    err
}

/// `host.table.append` with a `source_url` or `csv_text`.
pub async fn append_from_source(state: &AppState, tenant: &Tenant, args: &Value, source: Source) -> Result<Value, AppError> {
    let table = tables::arg_str(args, "table")?;
    let import_id = import_id_arg(args)?;
    let target = Target { tool: "host.table.append", table: &table, import_id: import_id.as_deref() };
    if let Some(id) = &import_id
        && let Some(first) = replay(state, tenant, target.tool, &table, id).await?
    {
        return Ok(first);
    }
    let path = tables::tenant_db_path(state, tenant.id);
    let probe = table.clone();
    tables::with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| tables::load_schema_sync(conn, &probe).map(|_| ())).await?;
    let loaded = match load(state, tenant, &source).await {
        Ok(l) => l,
        Err(failed) => {
            let (err, origin) = *failed;
            return Err(fail(state, tenant, &target, &origin, err).await);
        }
    };
    match write(state, tenant, &target, &loaded).await? {
        Wrote::Replayed(first) => Ok(first),
        Wrote::Rows(w) => {
            after_write(state, tenant, &table, false).await;
            Ok(result_json(&target, &w, &loaded.origin))
        }
    }
}

/// `host.table.create` with a `source_url` or `csv_text`: infers the columns,
/// creates the table, loads the rows; a failed load drops the new table again.
pub async fn create_from_source(state: &AppState, tenant: &Tenant, args: &Value, source: Source) -> Result<Value, AppError> {
    let name = tables::arg_str(args, "name")?;
    if !tables::is_valid_ident(&name) || name.starts_with(tables::HANDLE_PREFIX) {
        return Err(AppError::InvalidArgs(format!(
            "name: must match {} and not start with '{}'; got '{name}'",
            tables::IDENT_PATTERN,
            tables::HANDLE_PREFIX
        )));
    }
    let import_id = import_id_arg(args)?;
    let target = Target { tool: "host.table.create", table: &name, import_id: import_id.as_deref() };
    if let Some(id) = &import_id
        && let Some(first) = replay(state, tenant, target.tool, &name, id).await?
    {
        return Ok(first);
    }
    let loaded = match load(state, tenant, &source).await {
        Ok(l) => l,
        Err(failed) => {
            let (err, origin) = *failed;
            return Err(fail(state, tenant, &target, &origin, err).await);
        }
    };
    let columns = infer_columns(&loaded.parsed);
    let declared: Map<String, Value> = columns.iter().map(|(n, t)| (n.clone(), json!(t.as_str()))).collect();
    let mut create_args = json!({"name": name, "columns": declared});
    if let Some(pk) = args.get("primary_key") {
        create_args["primary_key"] = pk.clone();
    }
    if let Err(e) = tables::table_create_declared(state, tenant, &create_args).await {
        return Err(fail(state, tenant, &target, &loaded.origin, e).await);
    }
    match write(state, tenant, &target, &loaded).await {
        Ok(Wrote::Rows(w)) => {
            after_write(state, tenant, &name, true).await;
            Ok(result_json(&target, &w, &loaded.origin))
        }
        Ok(Wrote::Replayed(first)) => Ok(first),
        Err(e) => {
            if let Err(drop_err) = tables::table_drop(state, tenant, &json!({"name": name})).await {
                tracing::warn!(error = %drop_err, table = %name, "failed to drop a table whose import failed");
            }
            Err(fail(state, tenant, &target, &loaded.origin, e).await)
        }
    }
}
