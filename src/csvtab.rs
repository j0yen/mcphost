//! PRD-mcphost-csv-to-table-one-call: `host.table.from_blob` -- a CSV/TSV
//! already stored on the host becomes a typed, queryable table in one call.
//!
//! The parse / sniff / infer / convert core ([`load`]) is pure (bytes in,
//! typed rows and a type report out); [`table_from_blob`] is the thin async
//! wrapper that reads the stored bytes, checks the plan's quotas, and writes
//! through `tables::table_create` / `tables::table_append`, so a refused or
//! aborted load never leaves a table behind.
//!
//! The RFC 4180 reader is hand-rolled (quoted newlines, `""` escapes) rather
//! than adding a direct dependency: the lockfile has no `csv` crate today.

use crate::errors::AppError;
use crate::db::Tenant;
use crate::state::AppState;
use crate::tables::{self, ColumnType};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashSet};

/// R3: `skipped` lists at most this many entries; `rows_skipped` is the total.
pub const SKIPPED_CAP: usize = 100;
/// R2: inference votes over this many data rows unless `infer_rows` says more.
pub const INFER_ROWS_DEFAULT: usize = 1_000;
pub const INFER_ROWS_MAX: usize = 50_000;
const DELIMITERS: [char; 4] = [',', ';', '\t', '|'];
/// A type wins when at least this share of the sample's non-empty cells parse
/// as it, so one stray `n/a` is reported as a skipped cell instead of
/// demoting the whole column to text.
const VOTE_PERCENT: usize = 95;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsvType {
    Integer,
    Real,
    Text,
    Date,
    Bool,
}

impl CsvType {
    pub fn as_str(self) -> &'static str {
        match self {
            CsvType::Integer => "integer",
            CsvType::Real => "real",
            CsvType::Text => "text",
            CsvType::Date => "date",
            CsvType::Bool => "bool",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "integer" => Some(CsvType::Integer),
            "real" => Some(CsvType::Real),
            "text" => Some(CsvType::Text),
            "date" | "timestamp" => Some(CsvType::Date),
            "bool" | "boolean" => Some(CsvType::Bool),
            _ => None,
        }
    }

    /// The table-store column type this loads into.
    pub fn column_type(self) -> ColumnType {
        match self {
            CsvType::Integer => ColumnType::Integer,
            CsvType::Real => ColumnType::Real,
            CsvType::Text => ColumnType::Text,
            CsvType::Date => ColumnType::Timestamp,
            CsvType::Bool => ColumnType::Boolean,
        }
    }

    fn article(self) -> &'static str {
        match self {
            CsvType::Integer => "an",
            _ => "a",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Options {
    pub header: bool,
    pub delimiter: Option<char>,
    /// column (sanitised or source name) -> type name
    pub types: BTreeMap<String, String>,
    pub abort_on_error: bool,
    pub infer_rows: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            header: true,
            delimiter: None,
            types: BTreeMap::new(),
            abort_on_error: false,
            infer_rows: INFER_ROWS_DEFAULT,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Column {
    pub name: String,
    pub source_name: Option<String>,
    pub ty: CsvType,
    pub nulls: usize,
    pub distinct: usize,
    pub samples: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skip {
    pub row: usize,
    pub column: String,
    pub reason: String,
}

#[derive(Debug)]
pub struct Loaded {
    pub delimiter: char,
    pub columns: Vec<Column>,
    /// One object per loaded row; null cells are omitted (stored as NULL).
    pub rows: Vec<Map<String, Value>>,
    pub rows_skipped: usize,
    pub skipped: Vec<Skip>,
}

// ---- encoding ------------------------------------------------------------

/// R1: UTF-8, UTF-8 BOM, UTF-16 (BOM, or NUL-interleaved without one), else
/// Latin-1.
pub fn decode(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    let utf16 = |data: &[u8], le: bool| -> String {
        let units: Vec<u16> = data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| if le { u16::from_le_bytes(*c) } else { u16::from_be_bytes(*c) })
            .collect();
        String::from_utf16_lossy(&units)
    };
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(rest, true);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return utf16(rest, false);
    }
    if bytes.len() >= 4 {
        let (mut even0, mut odd0) = (0usize, 0usize);
        for (i, b) in bytes.iter().enumerate().take(4096) {
            if *b == 0 {
                if i % 2 == 0 { even0 += 1 } else { odd0 += 1 }
            }
        }
        let half = bytes.len().min(4096) / 2;
        if odd0 * 10 >= half * 3 && even0 == 0 {
            return utf16(bytes, true);
        }
        if even0 * 10 >= half * 3 && odd0 == 0 {
            return utf16(bytes, false);
        }
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|b| *b as char).collect(),
    }
}

// ---- record reader ------------------------------------------------------

/// `(1-based file line the record starts on, fields)`. Blank lines are
/// dropped; a quoted field may span lines.
fn parse_records(text: &str, delim: char) -> Vec<(usize, Vec<String>)> {
    let mut out = Vec::new();
    let mut fields: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut quoted_field = false;
    let mut in_quotes = false;
    let mut line = 1usize;
    let mut start_line = 1usize;
    let mut chars = text.chars().peekable();
    let mut at_field_start = true;

    let finish = |fields: &mut Vec<String>, field: &mut String, quoted: &mut bool, start: usize, out: &mut Vec<(usize, Vec<String>)>| {
        fields.push(std::mem::take(field));
        let blank = fields.len() == 1 && fields[0].is_empty() && !*quoted;
        *quoted = false;
        let rec = std::mem::take(fields);
        if !blank {
            out.push((start, rec));
        }
    };

    while let Some(c) = chars.next() {
        if in_quotes {
            match c {
                '"' => {
                    if chars.peek() == Some(&'"') {
                        chars.next();
                        field.push('"');
                    } else {
                        in_quotes = false;
                    }
                }
                '\n' => {
                    line += 1;
                    field.push('\n');
                }
                other => field.push(other),
            }
            continue;
        }
        match c {
            '"' if at_field_start => {
                in_quotes = true;
                quoted_field = true;
                at_field_start = false;
            }
            c if c == delim => {
                fields.push(std::mem::take(&mut field));
                at_field_start = true;
            }
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                finish(&mut fields, &mut field, &mut quoted_field, start_line, &mut out);
                line += 1;
                start_line = line;
                at_field_start = true;
            }
            '\n' => {
                finish(&mut fields, &mut field, &mut quoted_field, start_line, &mut out);
                line += 1;
                start_line = line;
                at_field_start = true;
            }
            other => {
                field.push(other);
                at_field_start = false;
            }
        }
    }
    if !field.is_empty() || !fields.is_empty() || quoted_field {
        finish(&mut fields, &mut field, &mut quoted_field, start_line, &mut out);
    }
    out
}

/// R1: the candidate whose field count is most consistent over the first 20
/// records wins (ties: the wider split, then `, ; \t |` order).
fn detect_delimiter(text: &str) -> char {
    let mut end = text.len().min(64 * 1024);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let prefix = &text[..end];
    let mut best = (0usize, 0usize, ',');
    for d in DELIMITERS {
        let recs = parse_records(prefix, d);
        let recs = &recs[..recs.len().min(20)];
        let Some((_, first)) = recs.first() else { continue };
        let n = first.len();
        if n < 2 {
            continue;
        }
        let consistent = recs.iter().filter(|(_, r)| r.len() == n).count();
        if (consistent, n) > (best.0, best.1) {
            best = (consistent, n, d);
        }
    }
    best.2
}

// ---- cell parsers --------------------------------------------------------

/// A leading zero followed by another digit (`00123`, `-007`) is an
/// identifier, not a number: it vetoes integer and real.
fn leading_zero_veto(s: &str) -> bool {
    let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
    let b = digits.as_bytes();
    b.len() > 1 && b[0] == b'0' && b[1].is_ascii_digit()
}

fn parse_int(s: &str) -> Option<i64> {
    if leading_zero_veto(s) {
        return None;
    }
    s.parse::<i64>().ok()
}

fn parse_real(s: &str) -> Option<f64> {
    if leading_zero_veto(s) || !s.bytes().all(|c| c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'E' | b'+' | b'-')) {
        return None;
    }
    s.parse::<f64>().ok().filter(|f| f.is_finite())
}

fn parse_bool_word(s: &str) -> Option<bool> {
    match s.to_ascii_lowercase().as_str() {
        "true" | "yes" => Some(true),
        "false" | "no" => Some(false),
        _ => None,
    }
}

fn digits(s: &str, n: usize) -> Option<u32> {
    (s.len() == n && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse().ok())?
}

fn valid_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let (Some(y), Some(m), Some(d)) = (digits(&s[0..4], 4), digits(&s[5..7], 2), digits(&s[8..10], 2)) else {
        return false;
    };
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let dim = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        _ => return false,
    };
    (1..=dim).contains(&d)
}

/// `YYYY-MM-DD`, or RFC 3339 `YYYY-MM-DDTHH:MM:SS[.f][Z|±HH:MM]`.
fn valid_date_or_datetime(s: &str) -> bool {
    if s.len() == 10 {
        return valid_date(s);
    }
    if s.len() < 19 || !s.is_char_boundary(10) || !valid_date(&s[..10]) || s.as_bytes()[10] != b'T' {
        return false;
    }
    let rest = &s[11..];
    let (time, zone) = match rest.find(['Z', '+', '-']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let (hms, frac) = match time.split_once('.') {
        Some((h, f)) => (h, Some(f)),
        None => (time, None),
    };
    let parts: Vec<&str> = hms.split(':').collect();
    if parts.len() != 3 {
        return false;
    }
    let ok = matches!(digits(parts[0], 2), Some(h) if h < 24)
        && matches!(digits(parts[1], 2), Some(m) if m < 60)
        && matches!(digits(parts[2], 2), Some(sec) if sec <= 60);
    let frac_ok = frac.is_none_or(|f| !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit()));
    let zone_ok = zone.is_empty()
        || zone == "Z"
        || (zone.len() == 6
            && matches!(zone.as_bytes()[0], b'+' | b'-')
            && zone.as_bytes()[3] == b':'
            && digits(&zone[1..3], 2).is_some_and(|h| h < 24)
            && digits(&zone[4..6], 2).is_some_and(|m| m < 60));
    ok && frac_ok && zone_ok
}

/// One cell -> JSON of the column's type, or the `reason` it was refused.
fn convert(cell: &str, ty: CsvType) -> Result<Value, String> {
    let fail = || format!("not {} {}: '{}'", ty.article(), ty.as_str(), cell);
    let t = cell.trim();
    match ty {
        CsvType::Text => Ok(json!(cell)),
        CsvType::Integer => parse_int(t).map(|i| json!(i)).ok_or_else(fail),
        CsvType::Real => parse_real(t).map(|f| json!(f)).ok_or_else(fail),
        CsvType::Bool => match t {
            "0" => Ok(json!(false)),
            "1" => Ok(json!(true)),
            _ => parse_bool_word(t).map(|b| json!(b)).ok_or_else(fail),
        },
        CsvType::Date => {
            if valid_date_or_datetime(t) {
                Ok(json!(t))
            } else {
                Err(fail())
            }
        }
    }
}

fn is_null_cell(cell: &str, ty: CsvType) -> bool {
    cell.is_empty() || (ty != CsvType::Text && cell.trim().is_empty())
}

fn infer_type(cells: &[&str]) -> CsvType {
    let vals: Vec<&str> = cells.iter().map(|c| c.trim()).filter(|c| !c.is_empty()).collect();
    let n = vals.len();
    if n == 0 {
        return CsvType::Text;
    }
    let wins = |count: usize| count * 100 >= n * VOTE_PERCENT;
    let n_int = vals.iter().filter(|v| parse_int(v).is_some()).count();
    let n_real = vals.iter().filter(|v| parse_real(v).is_some()).count();
    let n_word = vals.iter().filter(|v| parse_bool_word(v).is_some()).count();
    let n_01 = vals.iter().filter(|v| matches!(**v, "0" | "1")).count();
    let n_date = vals.iter().filter(|v| valid_date_or_datetime(v)).count();
    if wins(n_real) {
        return if n_int == n_real { CsvType::Integer } else { CsvType::Real };
    }
    if n_word > 0 && wins(n_word + n_01) {
        return CsvType::Bool;
    }
    if wins(n_date) {
        return CsvType::Date;
    }
    CsvType::Text
}

// ---- headers -------------------------------------------------------------

/// R2: the table identifier rule `^[A-Za-z_][A-Za-z0-9_]{0,63}$`.
fn sanitise(raw: &str, index: usize, taken: &HashSet<String>) -> String {
    let mut s: String = raw.trim().chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect();
    if s.is_empty() || s.chars().all(|c| c == '_') {
        s = format!("col{}", index + 1);
    }
    if s.as_bytes()[0].is_ascii_digit() {
        s.insert(0, '_');
    }
    s.truncate(60);
    let mut out = s.clone();
    let mut n = 2;
    while taken.contains(&out) {
        out = format!("{s}_{n}");
        n += 1;
    }
    out
}

fn col_cell(rec: &[String], i: usize) -> &str {
    rec.get(i).map(String::as_str).unwrap_or("")
}

// ---- the loader ----------------------------------------------------------

fn load_error(msg: String) -> AppError {
    AppError::InvalidArgs(msg)
}

/// Parse, infer, convert. With `abort_on_error` the first refused cell fails
/// the whole call (nothing has been written -- this function writes nothing).
pub fn load(bytes: &[u8], opts: &Options) -> Result<Loaded, AppError> {
    let text = decode(bytes);
    let delimiter = opts.delimiter.unwrap_or_else(|| detect_delimiter(&text));
    let mut records = parse_records(&text, delimiter);
    if records.is_empty() {
        return Err(load_error("csv: the input holds no rows".to_string()));
    }
    let width = records[0].1.len();
    let raw_names: Vec<String> = if opts.header {
        records.remove(0).1
    } else {
        (1..=width).map(|i| format!("col{i}")).collect()
    };
    let mut taken = HashSet::new();
    let mut names = Vec::with_capacity(raw_names.len());
    for (i, raw) in raw_names.iter().enumerate() {
        let name = sanitise(raw, i, &taken);
        taken.insert(name.clone());
        names.push(name);
    }
    let ncols = names.len();

    let mut forced: Vec<Option<CsvType>> = vec![None; ncols];
    for (col, ty) in &opts.types {
        let parsed = CsvType::parse(ty).ok_or_else(|| {
            load_error(format!("types: column '{col}' has unknown type '{ty}'; expected one of integer, real, text, date, bool"))
        })?;
        let idx = names.iter().position(|n| n == col).or_else(|| raw_names.iter().position(|n| n == col));
        match idx {
            Some(i) => forced[i] = Some(parsed),
            None => return Err(load_error(format!("types: no column named '{col}'"))),
        }
    }

    let sample_n = opts.infer_rows.clamp(1, INFER_ROWS_MAX).min(records.len());
    let mut tys = Vec::with_capacity(ncols);
    for (i, f) in forced.iter().enumerate() {
        let sample: Vec<&str> = records[..sample_n].iter().map(|(_, r)| col_cell(r, i)).collect();
        tys.push(f.unwrap_or_else(|| infer_type(&sample)));
    }

    let mut rows = Vec::with_capacity(records.len());
    let mut skipped = Vec::new();
    let mut rows_skipped = 0usize;
    let mut nulls = vec![0usize; ncols];
    let mut distinct: Vec<HashSet<&str>> = vec![HashSet::new(); ncols];
    let mut samples: Vec<Vec<String>> = vec![Vec::new(); ncols];
    'rows: for (ri, (line, rec)) in records.iter().enumerate() {
        let mut refusals: Vec<(String, String)> = Vec::new();
        if rec.len() != ncols {
            refusals.push((String::new(), format!("expected {ncols} fields, found {}", rec.len())));
        }
        let mut row = Map::new();
        if refusals.is_empty() {
            for i in 0..ncols {
                let cell = col_cell(rec, i);
                if is_null_cell(cell, tys[i]) {
                    continue;
                }
                match convert(cell, tys[i]) {
                    Ok(v) => {
                        row.insert(names[i].clone(), v);
                    }
                    Err(reason) => refusals.push((names[i].clone(), reason)),
                }
            }
        }
        if !refusals.is_empty() {
            if opts.abort_on_error {
                let (column, reason) = &refusals[0];
                return Err(load_error(format!(
                    "csv_load_aborted: row {line}, column '{column}': {reason}; no table was created"
                )));
            }
            rows_skipped += 1;
            for (column, reason) in refusals {
                if skipped.len() < SKIPPED_CAP {
                    skipped.push(Skip { row: *line, column, reason });
                }
            }
            continue 'rows;
        }
        for i in 0..ncols {
            let cell = col_cell(rec, i);
            if is_null_cell(cell, tys[i]) {
                nulls[i] += 1;
            } else if ri < INFER_ROWS_DEFAULT.max(sample_n) {
                distinct[i].insert(cell);
                if samples[i].len() < 3 && !samples[i].iter().any(|s| s == cell) {
                    samples[i].push(cell.to_string());
                }
            }
        }
        rows.push(row);
    }

    let columns = (0..ncols)
        .map(|i| Column {
            name: names[i].clone(),
            source_name: (names[i] != raw_names[i]).then(|| raw_names[i].clone()),
            ty: tys[i],
            nulls: nulls[i],
            distinct: distinct[i].len(),
            samples: std::mem::take(&mut samples[i]),
        })
        .collect();
    Ok(Loaded { delimiter, columns, rows, rows_skipped, skipped })
}

// ---- the verb ------------------------------------------------------------

fn report(name: &str, loaded: &Loaded, dry_run: bool) -> Value {
    let columns: Vec<Value> = loaded
        .columns
        .iter()
        .map(|c| {
            let mut o = json!({
                "name": c.name, "type": c.ty.as_str(), "nulls": c.nulls,
                "distinct": c.distinct, "samples": c.samples,
            });
            if let Some(src) = &c.source_name {
                o["source_name"] = json!(src);
            }
            o
        })
        .collect();
    let select: Vec<&str> = loaded.columns.iter().map(|c| c.name.as_str()).collect();
    json!({
        "table": name,
        "dry_run": dry_run,
        "delimiter": loaded.delimiter.to_string(),
        "rows_loaded": loaded.rows.len(),
        "rows_skipped": loaded.rows_skipped,
        "columns": columns,
        "preview": loaded.rows.iter().take(5).collect::<Vec<_>>(),
        "skipped": loaded.skipped.iter().map(|s| json!({"row": s.row, "column": s.column, "reason": s.reason})).collect::<Vec<_>>(),
        "sample_query": format!("SELECT {} FROM {name} LIMIT 5", select.join(", ")),
    })
}

fn options_from_args(args: &Value) -> Result<Options, AppError> {
    let mut o = Options::default();
    if let Some(h) = args.get("header") {
        o.header = h.as_bool().ok_or_else(|| load_error("header: must be a boolean".to_string()))?;
    }
    match args.get("delimiter").and_then(Value::as_str) {
        None | Some("auto") => {}
        Some("\\t") | Some("tab") => o.delimiter = Some('\t'),
        Some(s) if s.chars().count() == 1 && DELIMITERS.contains(&s.chars().next().unwrap()) => {
            o.delimiter = s.chars().next();
        }
        Some(other) => {
            return Err(load_error(format!("delimiter: '{other}' is not one of auto, ',', ';', tab, '|'")));
        }
    }
    if let Some(t) = args.get("types") {
        let obj = t.as_object().ok_or_else(|| load_error("types: must be an object of column -> type".to_string()))?;
        for (k, v) in obj {
            let ty = v.as_str().ok_or_else(|| load_error(format!("types: column '{k}' type must be a string")))?;
            o.types.insert(k.clone(), ty.to_string());
        }
    }
    match args.get("on_error").and_then(Value::as_str) {
        None | Some("skip") => {}
        Some("abort") => o.abort_on_error = true,
        Some(other) => return Err(load_error(format!("on_error: '{other}' is not one of skip, abort"))),
    }
    if let Some(n) = args.get("infer_rows").and_then(Value::as_u64) {
        o.infer_rows = (n as usize).clamp(1, INFER_ROWS_MAX);
    }
    Ok(o)
}

/// `host.table.from_blob {blob | doc, name, header?, delimiter?, types?,
/// mode?: create | replace, on_error?: skip | abort, infer_rows?, dry_run?}`.
pub async fn table_from_blob(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| load_error("missing required argument 'name'".to_string()))?
        .to_string();
    let opts = options_from_args(args)?;
    let blob = args.get("blob").and_then(Value::as_str);
    let doc = args.get("doc").and_then(Value::as_str);
    let mode = args.get("mode").and_then(Value::as_str).unwrap_or("create");
    if !matches!(mode, "create" | "replace") {
        return Err(load_error(format!("mode: '{mode}' is not one of create, replace")));
    }
    let dry_run = args.get("dry_run").and_then(Value::as_bool).unwrap_or(false);

    let plan = state
        .plans
        .get(&tenant.plan)
        .ok_or_else(|| AppError::Internal(format!("unknown plan '{}'", tenant.plan)))?
        .clone();
    let bytes = match (blob, doc) {
        (Some(b), None) => crate::blobs::blob_bytes(state, tenant, b).await?,
        (None, Some(d)) => crate::docs::doc_bytes(state, tenant, d).await?,
        _ => return Err(load_error("pass exactly one of 'blob' or 'doc'".to_string())),
    };
    if bytes.len() as i64 > plan.blob_max_bytes {
        return Err(crate::billing::quota_exceeded_for(
            &state.plans,
            state.billing_config.billing_mode(),
            &tenant.plan,
            "blob_max_bytes",
            plan.blob_max_bytes,
            bytes.len() as i64,
            None,
        ));
    }

    let loaded = load(&bytes, &opts)?;
    if dry_run {
        return Ok(report(&name, &loaded, true));
    }

    // R4: refuse before anything is written, with {limit, used, found}.
    let found = loaded.rows.len() as i64;
    if found > plan.table_rows_max {
        let mut err = crate::billing::quota_exceeded_for(
            &state.plans,
            state.billing_config.billing_mode(),
            &tenant.plan,
            "table_rows_max",
            plan.table_rows_max,
            0,
            None,
        );
        if let AppError::Structured { data, .. } = &mut err
            && let Some(o) = data.as_object_mut()
        {
            o.insert("found".to_string(), json!(found));
        }
        return Err(err);
    }
    let est_bytes: i64 = loaded.rows.iter().map(|r| serde_json::to_string(r).map(|s| s.len() as i64).unwrap_or(0)).sum();
    if est_bytes > plan.table_bytes_max {
        return Err(crate::billing::quota_exceeded_for(
            &state.plans,
            state.billing_config.billing_mode(),
            &tenant.plan,
            "table_bytes_max",
            plan.table_bytes_max,
            0,
            None,
        ));
    }

    if mode == "replace" {
        let listed = tables::table_list(state, tenant, &json!({})).await?;
        let exists = listed["tables"].as_array().is_some_and(|t| t.iter().any(|x| x["name"] == json!(name)));
        if exists {
            tables::table_drop(state, tenant, &json!({"name": name, "confirm": true})).await?;
        }
    }
    let columns: Map<String, Value> =
        loaded.columns.iter().map(|c| (c.name.clone(), json!(c.ty.column_type().as_str()))).collect();
    tables::table_create(state, tenant, &json!({"name": name, "columns": columns})).await?;
    if !loaded.rows.is_empty() {
        let appended = tables::table_append(state, tenant, &json!({"table": name, "rows": loaded.rows})).await;
        if let Err(e) = appended {
            let _ = tables::table_drop(state, tenant, &json!({"name": name, "confirm": true})).await;
            return Err(e);
        }
    }
    Ok(report(&name, &loaded, false))
}
