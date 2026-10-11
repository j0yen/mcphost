//! PRD-mcphost-csv-to-table-one-call: `host.table.from_blob` -- a CSV or TSV
//! already stored on the host becomes a typed, queryable table in one call,
//! with a preview and a type report.
//!
//! Layout: [`TableFromBlobArgs`] + [`ARG_SPECS`] are the one source of the
//! verb's args schema AND the llms.txt recipe's argument table (R6, rendered
//! by `gendocs`); the parser ([`decode_text`], [`detect_delimiter`],
//! [`parse_records`]), the inference ([`infer_column`]) and the cell
//! conversion ([`convert_cell`]) are pure and unit-tested here; the verb
//! itself ([`table_from_blob`]) does the quota checks, then creates the table
//! and loads every row in ONE transaction through `tables::insert_rows_sync`
//! (`host.table.append`'s own path).

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::errors::AppError;
use crate::state::AppState;
use crate::db::Tenant;
use crate::tables::{self, ColumnType, META_TABLE};

/// Rows sampled for type inference unless `infer_rows` says otherwise (R2).
pub const DEFAULT_INFER_ROWS: usize = 1_000;
/// Ceiling on `infer_rows` (the PRD's open-question default).
pub const MAX_INFER_ROWS: usize = 50_000;
/// `skipped` carries at most this many entries; `rows_skipped` is the total (R3).
pub const SKIPPED_CAP: usize = 100;
/// Rows echoed in `preview` (R5).
pub const PREVIEW_ROWS: usize = 5;
/// `columns[].samples` length cap (R5).
pub const SAMPLES_PER_COLUMN: usize = 3;
/// Delimiters tried on the first lines of the file, in tie-break order (R1).
pub const DELIMITERS: [char; 4] = [',', ';', '\t', '|'];
/// The column types a report and a `types` override speak (R2).
pub const TYPE_NAMES: [&str; 5] = ["integer", "real", "text", "date", "bool"];

/// The verb's arguments. Field order = [`ARG_SPECS`] order; the unit test
/// `arg_specs_match_the_struct_fields` fails if the two drift.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TableFromBlobArgs {
    #[serde(default)]
    pub blob: Option<String>,
    #[serde(default)]
    pub doc: Option<String>,
    pub name: String,
    #[serde(default = "default_true")]
    pub header: bool,
    #[serde(default)]
    pub delimiter: Option<String>,
    #[serde(default)]
    pub types: Option<BTreeMap<String, String>>,
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default = "default_on_error")]
    pub on_error: String,
    #[serde(default)]
    pub infer_rows: Option<usize>,
    #[serde(default)]
    pub dry_run: bool,
    /// Accepted (and ignored here) because every `host.*` tool schema
    /// carries it; the dispatcher resolves the tenant before this runs.
    #[serde(default)]
    pub tenant_key: Option<String>,
}

fn default_true() -> bool {
    true
}
fn default_mode() -> String {
    "create".to_string()
}
fn default_on_error() -> String {
    "skip".to_string()
}

/// One argument of the verb: the single descriptor both the JSON schema and
/// the llms.txt argument table are rendered from.
pub struct ArgSpec {
    pub name: &'static str,
    pub json_type: &'static str,
    pub required: bool,
    pub default: &'static str,
    pub description: &'static str,
}

/// Every argument except `tenant_key` (added to all `host.*` schemas by the
/// handler's `host_schema`).
pub const ARG_SPECS: &[ArgSpec] = &[
    ArgSpec {
        name: "blob",
        json_type: "string",
        required: false,
        default: "-",
        description: "Id or name of a stored CSV/TSV to load. Exactly one of blob or doc.",
    },
    ArgSpec {
        name: "doc",
        json_type: "string",
        required: false,
        default: "-",
        description: "Id or name of a host.docs.put text/csv document to load. Exactly one of blob or doc.",
    },
    ArgSpec {
        name: "name",
        json_type: "string",
        required: true,
        default: "-",
        description: "Name of the table to create.",
    },
    ArgSpec {
        name: "header",
        json_type: "boolean",
        required: false,
        default: "true",
        description: "The first line holds column names. false names the columns col1, col2, ...",
    },
    ArgSpec {
        name: "delimiter",
        json_type: "string",
        required: false,
        default: "auto",
        description: "One character: , ; tab | -- auto-detected from the first 20 lines when omitted.",
    },
    ArgSpec {
        name: "types",
        json_type: "object",
        required: false,
        default: "inferred",
        description: "Per-column override, {column: integer|real|text|date|bool}; only those columns skip inference.",
    },
    ArgSpec {
        name: "mode",
        json_type: "string",
        required: false,
        default: "create",
        description: "create (refuse an existing table) | replace (drop and recreate) | append (load into an existing table; columns match by name, never altered).",
    },
    ArgSpec {
        name: "on_error",
        json_type: "string",
        required: false,
        default: "skip",
        description: "skip leaves a bad row out and reports it; abort refuses the whole load and creates nothing.",
    },
    ArgSpec {
        name: "infer_rows",
        json_type: "integer",
        required: false,
        default: "1000",
        description: "Data rows sampled for type inference, 1 to 50000.",
    },
    ArgSpec {
        name: "dry_run",
        json_type: "boolean",
        required: false,
        default: "false",
        description: "Return the type report and preview without creating or writing anything.",
    },
];

/// Verb name, one place.
pub const VERB: &str = "host.table.from_blob";

/// One-line description of the verb (tools/list text).
pub const VERB_DESCRIPTION: &str = "Load a CSV or TSV already stored on the host (blob or doc) into a new typed table in one \
call: infers integer|real|text|date|bool per column over the first 1,000 rows, loads every row that \
parses, and returns {table, rows_loaded, rows_skipped, columns, preview, skipped, sample_query}. A bad \
cell is skipped and named by row and column (or aborts the load with on_error=abort). Checked against \
the plan's table quotas before any row is written.";

/// The JSON-schema `properties` object of the verb, rendered from [`ARG_SPECS`].
pub fn schema_properties() -> Value {
    let mut props = Map::new();
    for a in ARG_SPECS {
        let mut p = Map::new();
        p.insert("type".to_string(), json!(a.json_type));
        p.insert("description".to_string(), json!(a.description));
        props.insert(a.name.to_string(), Value::Object(p));
    }
    Value::Object(props)
}

/// Names of the required arguments, from [`ARG_SPECS`].
pub fn required_args() -> Vec<&'static str> {
    ARG_SPECS.iter().filter(|a| a.required).map(|a| a.name).collect()
}

/// The markdown argument table spliced into `www/llms.txt` between the
/// `csvtab` markers (and the source of `docs`' verb schema region).
pub fn render_args_table() -> String {
    let mut out = String::from("| argument | type | default | meaning |\n|---|---|---|---|\n");
    for a in ARG_SPECS {
        let req = if a.required { " (required)" } else { "" };
        out.push_str(&format!(
            "| `{}`{req} | {} | {} | {} |\n",
            a.name, a.json_type, a.default, a.description
        ));
    }
    out
}

/// One line per argument group for the llms.txt recipe (`name:type=default`),
/// generated from [`ARG_SPECS`] so the recipe cannot name an argument the
/// verb does not take.
pub fn render_args_line() -> String {
    ARG_SPECS
        .iter()
        .map(|a| {
            if a.required {
                format!("`{}` ({}, required)", a.name, a.json_type)
            } else if a.default == "-" {
                format!("`{}` ({})", a.name, a.json_type)
            } else {
                format!("`{}` ({}, default {})", a.name, a.json_type, a.default)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The llms.txt recipe region (3-space indented, it sits inside a numbered
/// step): the CSV-in-a-document path collapses declare + append into one
/// `from_blob` call, with the argument list generated from [`ARG_SPECS`].
pub fn render_recipe_section() -> String {
    let mut out = String::new();
    out.push_str("   CSV already stored as a document (`host.docs.put`, mime `text/csv`)? One call replaces\n");
    out.push_str("   the declare and append steps below -- it infers a type per column and loads every row:\n");
    out.push_str("   ```\n");
    out.push_str("   host.table.from_blob(doc=\"expenses.csv\", name=\"expenses\")\n");
    out.push_str("   ```\n");
    out.push_str("   Returns `rows_loaded`, `rows_skipped`, `columns` (`name`, `type`, `nulls`, `distinct`,\n");
    out.push_str("   `samples`), a 5-row `preview`, `skipped` (`row`, `column`, `reason`; at most 100) and a\n");
    out.push_str("   `sample_query`. A bad cell is skipped and named, or `on_error=abort` loads nothing.\n");
    out.push_str("   Refusals: `csv_empty`, `csv_no_rows`, `quota_table_rows` (`limit`, `used`, `found`),\n");
    out.push_str("   `args_invalid`. Arguments: ");
    out.push_str(&render_args_line());
    out.push_str(".\n");
    out
}

pub const RECIPE_START: &str = "<!-- csvtab:start -->";
pub const RECIPE_END: &str = "<!-- csvtab:end -->";

/// Replace every region bracketed by [`RECIPE_START`]/[`RECIPE_END`] in
/// `content` with [`render_recipe_section`]; a doc without markers is
/// returned unchanged. Same shape as `install_links::splice_install_links_section`.
pub fn splice_recipe_section(content: &str) -> String {
    let section = format!("{RECIPE_START}\n{}\n{RECIPE_END}", render_recipe_section().trim_end());
    let mut out = String::new();
    let mut rest = content;
    while let (Some(start), Some(end_pos)) = (rest.find(RECIPE_START), rest.find(RECIPE_END)) {
        if end_pos <= start {
            break;
        }
        out.push_str(&rest[..start]);
        out.push_str(&section);
        rest = &rest[end_pos + RECIPE_END.len()..];
    }
    out.push_str(rest);
    out
}

/// The committed args-schema file (`schemas/table-from-blob.args.json`).
pub const SCHEMA_FILE: &str = "schemas/table-from-blob.args.json";

/// Pretty JSON of the full args schema, rendered from [`ARG_SPECS`].
pub fn render_schema_file() -> String {
    let doc = json!({
        "$comment": "Generated by `mcphost gen-docs` from csvtab::ARG_SPECS. Do not hand-edit.",
        "title": VERB,
        "type": "object",
        "properties": schema_properties(),
        "required": required_args(),
        "additionalProperties": false,
    });
    let mut s = serde_json::to_string_pretty(&doc).unwrap_or_default();
    s.push('\n');
    s
}

/// Names of the drifted regions among the committed `llms_txt` and `schema`
/// file contents (empty = in sync). `gendocs run --check` prints each.
pub fn drifted_regions(llms_txt: &str, schema_file: &str) -> Vec<String> {
    let mut out = Vec::new();
    if splice_recipe_section(llms_txt) != llms_txt {
        out.push("www/llms.txt region csvtab (database-in-a-minute recipe)".to_string());
    }
    if schema_file != render_schema_file() {
        out.push(format!("{SCHEMA_FILE} (host.table.from_blob args schema)"));
    }
    out
}

// ---- decoding ---------------------------------------------------------------

/// R1: UTF-8, UTF-8 BOM, UTF-16 (BOM, or NUL-interleaved without one), else
/// Latin-1. Returns the decoded text and the encoding label.
pub fn decode_text(bytes: &[u8]) -> (String, &'static str) {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return (String::from_utf8_lossy(rest).into_owned(), "utf-8-bom");
    }
    let utf16 = |data: &[u8], le: bool| -> String {
        let units = data.as_chunks::<2>().0.iter().map(|c| {
            if le {
                u16::from_le_bytes([c[0], c[1]])
            } else {
                u16::from_be_bytes([c[0], c[1]])
            }
        });
        char::decode_utf16(units)
            .map(|r| r.unwrap_or('\u{FFFD}'))
            .collect()
    };
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return (utf16(rest, true), "utf-16le");
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return (utf16(rest, false), "utf-16be");
    }
    // No BOM: UTF-16 text of mostly ASCII has a NUL in every other byte.
    if bytes.len() >= 4 {
        let sample = &bytes[..bytes.len().min(512)];
        let even_nul = sample.iter().step_by(2).filter(|b| **b == 0).count();
        let odd_nul = sample.iter().skip(1).step_by(2).filter(|b| **b == 0).count();
        let half = sample.len() / 2;
        if odd_nul * 2 > half && even_nul == 0 {
            return (utf16(bytes, true), "utf-16le");
        }
        if even_nul * 2 > half && odd_nul == 0 {
            return (utf16(bytes, false), "utf-16be");
        }
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => (s.to_string(), "utf-8"),
        Err(_) => (bytes.iter().map(|b| *b as char).collect(), "latin-1"),
    }
}

/// R1: the delimiter whose per-line count (outside quotes) is the most
/// consistent and non-zero over the first 20 non-blank lines; ties go to
/// the order of [`DELIMITERS`]. No candidate at all (a one-column file)
/// falls back to a comma.
pub fn detect_delimiter(text: &str) -> char {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).take(20).collect();
    let mut best: Option<(usize, usize, char)> = None;
    for d in DELIMITERS {
        let counts: Vec<usize> = lines
            .iter()
            .map(|l| {
                let mut in_q = false;
                let mut n = 0;
                for ch in l.chars() {
                    if ch == '"' {
                        in_q = !in_q;
                    } else if ch == d && !in_q {
                        n += 1;
                    }
                }
                n
            })
            .collect();
        let mut freq: BTreeMap<usize, usize> = BTreeMap::new();
        for c in counts.iter().filter(|c| **c > 0) {
            *freq.entry(*c).or_default() += 1;
        }
        let Some((&count, &lines_agreeing)) = freq.iter().max_by_key(|(c, n)| (**n, **c)) else {
            continue;
        };
        let cand = (lines_agreeing, count, d);
        if best.is_none_or(|b| (cand.0, cand.1) > (b.0, b.1)) {
            best = Some(cand);
        }
    }
    best.map(|b| b.2).unwrap_or(',')
}

/// One CSV record: its 1-based starting file line and its fields.
pub type Record = (usize, Vec<String>);

/// RFC 4180-style parse: quoted fields may hold the delimiter, doubled
/// quotes and newlines; `\r\n`, `\n` and a lone `\r` end a record; blank
/// lines (including a trailing one) produce no record. Each record carries
/// the file line it STARTED on (R3's `row`).
pub fn parse_records(text: &str, delim: char) -> Vec<Record> {
    let mut records = Vec::new();
    let mut fields: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut field_was_quoted = false;
    let mut line = 1usize;
    let mut record_line = 1usize;
    let mut chars = text.chars().peekable();

    let finish = |fields: &mut Vec<String>, field: &mut String, quoted: bool, records: &mut Vec<Record>, at: usize| {
        let blank = fields.is_empty() && field.is_empty() && !quoted;
        if blank {
            return;
        }
        fields.push(std::mem::take(field));
        records.push((at, std::mem::take(fields)));
    };

    while let Some(ch) = chars.next() {
        if in_quotes {
            match ch {
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
        match ch {
            '"' if field.is_empty() => {
                in_quotes = true;
                field_was_quoted = true;
            }
            c if c == delim => {
                fields.push(std::mem::take(&mut field));
                field_was_quoted = false;
            }
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                finish(&mut fields, &mut field, field_was_quoted, &mut records, record_line);
                field_was_quoted = false;
                line += 1;
                record_line = line;
            }
            '\n' => {
                finish(&mut fields, &mut field, field_was_quoted, &mut records, record_line);
                field_was_quoted = false;
                line += 1;
                record_line = line;
            }
            other => field.push(other),
        }
    }
    finish(&mut fields, &mut field, field_was_quoted, &mut records, record_line);
    records
}

// ---- inference --------------------------------------------------------------

/// A column type the loader speaks (R2) and how it maps onto a table column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsvType {
    Integer,
    Real,
    Text,
    Date,
    Bool,
}

impl CsvType {
    pub fn name(self) -> &'static str {
        match self {
            CsvType::Integer => "integer",
            CsvType::Real => "real",
            CsvType::Text => "text",
            CsvType::Date => "date",
            CsvType::Bool => "bool",
        }
    }

    /// Parses a type name from a `types` override; `timestamp`/`boolean`
    /// are accepted as the `host.table.create` spellings.
    pub fn parse(s: &str) -> Option<CsvType> {
        match s {
            "integer" => Some(CsvType::Integer),
            "real" => Some(CsvType::Real),
            "text" => Some(CsvType::Text),
            "date" | "timestamp" => Some(CsvType::Date),
            "bool" | "boolean" => Some(CsvType::Bool),
            _ => None,
        }
    }

    pub fn column_type(self) -> ColumnType {
        match self {
            CsvType::Integer => ColumnType::Integer,
            CsvType::Real => ColumnType::Real,
            CsvType::Text => ColumnType::Text,
            CsvType::Date => ColumnType::Timestamp,
            CsvType::Bool => ColumnType::Boolean,
        }
    }

    fn from_column_type(t: ColumnType) -> CsvType {
        match t {
            ColumnType::Integer => CsvType::Integer,
            ColumnType::Real => CsvType::Real,
            ColumnType::Boolean => CsvType::Bool,
            ColumnType::Timestamp => CsvType::Date,
            ColumnType::Text | ColumnType::Json => CsvType::Text,
        }
    }
}

fn has_leading_zero(s: &str) -> bool {
    let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
    digits.len() > 1 && digits.starts_with('0') && digits.as_bytes()[1].is_ascii_digit()
}

fn parse_integer(s: &str) -> Option<i64> {
    if has_leading_zero(s) {
        return None;
    }
    s.parse::<i64>().ok()
}

fn parse_real(s: &str) -> Option<f64> {
    if has_leading_zero(s) {
        return None;
    }
    // Rust's f64 parser accepts "inf"/"nan"/"infinity"; those are words.
    let first = s.chars().next()?;
    if !(first.is_ascii_digit() || matches!(first, '+' | '-' | '.')) {
        return None;
    }
    s.parse::<f64>().ok().filter(|f| f.is_finite())
}

fn parse_bool_word(s: &str) -> Option<bool> {
    match s.to_ascii_lowercase().as_str() {
        "true" | "yes" | "1" => Some(true),
        "false" | "no" | "0" => Some(false),
        _ => None,
    }
}

fn is_bool_word(s: &str) -> bool {
    matches!(s.to_ascii_lowercase().as_str(), "true" | "false" | "yes" | "no")
}

/// `YYYY-MM-DD` or an RFC 3339 datetime; nothing else is a date (the PRD's
/// technical considerations).
fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    let ymd = |b: &[u8]| -> bool {
        b.len() == 10
            && b[4] == b'-'
            && b[7] == b'-'
            && b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
            && {
                let m: u32 = s[5..7].parse().unwrap_or(0);
                let d: u32 = s[8..10].parse().unwrap_or(0);
                (1..=12).contains(&m) && (1..=31).contains(&d)
            }
    };
    if ymd(b) {
        return true;
    }
    if b.len() < 20 || !ymd(&b[..10]) || !matches!(b[10], b'T' | b't' | b' ') {
        return false;
    }
    let t = &s[11..];
    let tb = t.as_bytes();
    let hms = tb.len() >= 8
        && tb[2] == b':'
        && tb[5] == b':'
        && [0, 1, 3, 4, 6, 7].iter().all(|i| tb[*i].is_ascii_digit());
    if !hms {
        return false;
    }
    let (h, m, sec): (u32, u32, u32) = (
        t[0..2].parse().unwrap_or(99),
        t[3..5].parse().unwrap_or(99),
        t[6..8].parse().unwrap_or(99),
    );
    if h > 23 || m > 59 || sec > 60 {
        return false;
    }
    let mut rest = &t[8..];
    if let Some(frac) = rest.strip_prefix('.') {
        let n = frac.bytes().take_while(u8::is_ascii_digit).count();
        if n == 0 {
            return false;
        }
        rest = &frac[n..];
    }
    if matches!(rest, "Z" | "z") {
        return true;
    }
    let rb = rest.as_bytes();
    rb.len() == 6
        && matches!(rb[0], b'+' | b'-')
        && rb[3] == b':'
        && [1, 2, 4, 5].iter().all(|i| rb[*i].is_ascii_digit())
}

/// Votes of one column's sampled, non-empty cells.
#[derive(Default)]
struct Votes {
    seen: usize,
    integer: usize,
    real: usize,
    boolean: usize,
    bool_word: usize,
    date: usize,
}

/// A column with `seen / TOLERANCE_DIVISOR` or fewer stray cells still takes the
/// type of the rest (AC3: one `n/a` at line 412 of a 1,000-row file).
pub const TOLERANCE_DIVISOR: usize = 100;

/// R2: all-integer -> `integer`; all-real -> `real`; all-bool (with at
/// least one true/false/yes/no word, so a plain 0/1 column stays `integer`)
/// -> `bool`; all-date -> `date`; anything else, and a column with no
/// non-empty cell, -> `text`. Empty cells do not vote.
pub fn infer_column(cells: &[&str]) -> CsvType {
    let mut v = Votes::default();
    for raw in cells {
        let c = raw.trim();
        if c.is_empty() {
            continue;
        }
        v.seen += 1;
        if parse_integer(c).is_some() {
            v.integer += 1;
        }
        if parse_real(c).is_some() {
            v.real += 1;
        }
        if parse_bool_word(c).is_some() {
            v.boolean += 1;
        }
        if is_bool_word(c) {
            v.bool_word += 1;
        }
        if is_date(c) {
            v.date += 1;
        }
    }
    if v.seen == 0 {
        return CsvType::Text;
    }
    // Strict pass: every sampled cell parses.
    if v.boolean == v.seen && v.bool_word > 0 {
        return CsvType::Bool;
    }
    if v.integer == v.seen {
        return CsvType::Integer;
    }
    if v.real == v.seen {
        return CsvType::Real;
    }
    if v.date == v.seen {
        return CsvType::Date;
    }
    // Tolerant pass: with at least 100 sampled cells, up to 1% may be strays
    // (a stray `n/a` in a numeric column is skipped and named per row, R3,
    // instead of demoting the whole column to text).
    let slack = v.seen / TOLERANCE_DIVISOR;
    if slack == 0 {
        return CsvType::Text;
    }
    let within = |n: usize| n + slack >= v.seen;
    if v.integer > 0 && v.integer == v.real && within(v.integer) {
        CsvType::Integer
    } else if v.real > 0 && within(v.real) {
        CsvType::Real
    } else if v.bool_word > 0 && within(v.boolean) {
        CsvType::Bool
    } else if v.date > 0 && within(v.date) {
        CsvType::Date
    } else {
        CsvType::Text
    }
}

/// One cell converted to the JSON value its column stores, or the R3
/// `reason` it cannot be. An empty cell is `Null` for every type.
pub fn convert_cell(ty: CsvType, raw: &str) -> Result<Value, String> {
    if raw.trim().is_empty() {
        return Ok(Value::Null);
    }
    let c = raw.trim();
    match ty {
        CsvType::Text => Ok(Value::String(raw.to_string())),
        // A leading zero (`00123`) is an identifier, not a number: refused
        // loudly rather than stored as 123 (override with `types: {col: text}`).
        CsvType::Integer => parse_integer(c)
            .map(|i| json!(i))
            .ok_or_else(|| format!("not an integer: '{c}'")),
        CsvType::Real => c
            .parse::<f64>()
            .ok()
            .filter(|f| f.is_finite() && c.chars().next().is_some_and(|x| x.is_ascii_digit() || matches!(x, '+' | '-' | '.')))
            .map(|f| json!(f))
            .ok_or_else(|| format!("not a real: '{c}'")),
        CsvType::Bool => parse_bool_word(c)
            .map(Value::Bool)
            .ok_or_else(|| format!("not a bool: '{c}'")),
        CsvType::Date => {
            if is_date(c) {
                Ok(Value::String(c.to_string()))
            } else {
                Err(format!("not a date: '{c}'"))
            }
        }
    }
}

/// R2: a header cell as a table identifier -- non `[A-Za-z0-9_]` becomes
/// `_`, a leading digit gets a `c_` prefix, an empty name becomes `colN`,
/// and a duplicate gets `_2`, `_3`, ...; at most 64 characters.
pub fn sanitise_header(raw: &str, index: usize, taken: &BTreeSet<String>) -> String {
    let mut s: String = raw
        .trim()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect();
    let s_trim = s.trim_matches('_').to_string();
    if s_trim.is_empty() {
        s = format!("col{}", index + 1);
    } else {
        s = if raw.trim().chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            s
        } else {
            s_trim
        };
    }
    if s.starts_with(|c: char| c.is_ascii_digit()) {
        s = format!("c_{s}");
    }
    s.truncate(60);
    let base = s.clone();
    let mut n = 2;
    while taken.contains(&s) {
        s = format!("{base}_{n}");
        n += 1;
    }
    s
}

// ---- errors -----------------------------------------------------------------

fn structured(code: &'static str, message: String, data: Value) -> AppError {
    AppError::Structured { code, message, data }
}

fn quota_refusal(code: &'static str, limit_name: &str, limit: i64, used: i64, found: i64) -> AppError {
    structured(
        code,
        format!("loading would exceed {limit_name}: limit {limit}, used {used}, found {found}"),
        json!({"limit": limit, "used": used, "found": found, "limit_name": limit_name}),
    )
}

// ---- the verb ---------------------------------------------------------------

/// Resolves the CSV bytes of `blob` / `doc`. This host stores uploaded bytes
/// in the document store (`src/blobs.rs` from PRD-mcphost-blob-put-get-signed-url
/// is not in this tree), so both arguments resolve through it, tenant-scoped.
async fn resolve_bytes(state: &AppState, tenant: &Tenant, key: &str, which: &str) -> Result<Vec<u8>, AppError> {
    use base64::Engine as _;
    // An id (ULID) first, then a name -- the same two spellings host.docs.get takes.
    for arg in ["id", "name"] {
        let got = crate::docs::doc_get(state, tenant, &json!({ arg: key })).await;
        match got {
            Ok(v) => {
                let mime = v.get("mime").and_then(Value::as_str).unwrap_or("");
                if mime != "text/csv" {
                    return Err(AppError::InvalidArgs(format!(
                        "{which}: '{key}' is {mime}, not text/csv"
                    )));
                }
                let b64 = v.get("content_base64").and_then(Value::as_str).unwrap_or("");
                return base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .map_err(|e| AppError::Internal(format!("stored content is not base64: {e}")));
            }
            Err(e) if e.code() == "docs_not_found" => continue,
            Err(e) => return Err(e),
        }
    }
    Err(structured(
        "docs_not_found",
        format!("no {which} found for '{key}'"),
        json!({which: key}),
    ))
}

struct Loaded {
    names: Vec<String>,
    source_names: Vec<String>,
    types: Vec<CsvType>,
    rows: Vec<Map<String, Value>>,
    nulls: Vec<usize>,
    distinct: Vec<BTreeSet<String>>,
    samples: Vec<Vec<Value>>,
    skipped: Vec<Value>,
    rows_skipped: usize,
    found: usize,
}

/// `host.table.from_blob`.
pub async fn table_from_blob(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let a: TableFromBlobArgs = serde_json::from_value(args.clone())
        .map_err(|e| AppError::InvalidArgs(format!("host.table.from_blob: {e}")))?;
    if a.blob.is_some() == a.doc.is_some() {
        return Err(AppError::InvalidArgs(
            "exactly one of 'blob' or 'doc' is required (got neither or both)".to_string(),
        ));
    }
    if !tables::is_valid_ident(&a.name) || a.name.starts_with(tables::HANDLE_PREFIX) {
        return Err(AppError::InvalidArgs(format!(
            "name: must match ^[A-Za-z_][A-Za-z0-9_]{{0,63}}$ and not start with '{}'; got '{}'",
            tables::HANDLE_PREFIX,
            a.name
        )));
    }
    if !matches!(a.mode.as_str(), "create" | "replace" | "append") {
        return Err(AppError::InvalidArgs(format!(
            "mode: must be create, replace or append; got '{}'",
            a.mode
        )));
    }
    let abort = match a.on_error.as_str() {
        "skip" => false,
        "abort" => true,
        other => {
            return Err(AppError::InvalidArgs(format!(
                "on_error: must be skip or abort; got '{other}'"
            )))
        }
    };
    let infer_rows = a.infer_rows.unwrap_or(DEFAULT_INFER_ROWS);
    if infer_rows == 0 || infer_rows > MAX_INFER_ROWS {
        return Err(AppError::InvalidArgs(format!(
            "infer_rows: must be between 1 and {MAX_INFER_ROWS}; got {infer_rows}"
        )));
    }
    let mut overrides: BTreeMap<String, CsvType> = BTreeMap::new();
    for (col, ty) in a.types.iter().flatten() {
        let t = CsvType::parse(ty).ok_or_else(|| {
            AppError::InvalidArgs(format!(
                "types: column '{col}' has unknown type '{ty}'; expected one of {}",
                TYPE_NAMES.join(", ")
            ))
        })?;
        overrides.insert(col.clone(), t);
    }
    let delimiter: Option<char> = match a.delimiter.as_deref() {
        None | Some("auto") | Some("") => None,
        Some("\\t") | Some("tab") | Some("\t") => Some('\t'),
        Some(s) if s.chars().count() == 1 && DELIMITERS.contains(&s.chars().next().unwrap_or(',')) => s.chars().next(),
        Some(s) => {
            return Err(AppError::InvalidArgs(format!(
                "delimiter: must be one of , ; tab | or auto; got '{s}'"
            )))
        }
    };

    let (key, which) = match (&a.blob, &a.doc) {
        (Some(b), _) => (b.clone(), "blob"),
        (_, Some(d)) => (d.clone(), "doc"),
        _ => unreachable!("exactly-one check above"),
    };
    let bytes = resolve_bytes(state, tenant, &key, which).await?;
    if bytes.iter().all(|b| b.is_ascii_whitespace()) {
        return Err(structured(
            "csv_empty",
            format!("{which} '{key}' is empty"),
            json!({which: key, "bytes": bytes.len()}),
        ));
    }

    let (text, encoding) = decode_text(&bytes);
    let delim = delimiter.unwrap_or_else(|| detect_delimiter(&text));
    let mut records = parse_records(&text, delim);
    if records.is_empty() {
        return Err(structured(
            "csv_empty",
            format!("{which} '{key}' has no content"),
            json!({which: key, "bytes": bytes.len()}),
        ));
    }

    let header_rec = if a.header { Some(records.remove(0)) } else { None };
    let width = header_rec
        .as_ref()
        .map(|h| h.1.len())
        .unwrap_or_else(|| records.iter().map(|r| r.1.len()).max().unwrap_or(1));
    let mut names: Vec<String> = Vec::with_capacity(width);
    let mut source_names: Vec<String> = Vec::with_capacity(width);
    let mut taken: BTreeSet<String> = BTreeSet::new();
    for i in 0..width {
        let raw = header_rec.as_ref().map(|h| h.1[i].as_str()).unwrap_or("");
        let n = if header_rec.is_some() {
            sanitise_header(raw, i, &taken)
        } else {
            format!("col{}", i + 1)
        };
        taken.insert(n.clone());
        names.push(n);
        source_names.push(raw.trim().to_string());
    }
    // `types` keys may name a column by its sanitised or its source name.
    for key in overrides.keys() {
        if !names.contains(key) && !source_names.contains(key) {
            return Err(AppError::InvalidArgs(format!(
                "types: no column '{key}' in the file (columns: {})",
                names.join(", ")
            )));
        }
    }

    let found = records.len();
    let plan = state
        .plans
        .get(&tenant.plan)
        .ok_or_else(|| AppError::Internal(format!("tenant's plan '{}' is not in the loaded plan catalog", tenant.plan)))?;
    let rows_max = plan.table_rows_max;
    let bytes_max = plan.table_bytes_max;
    let tables_max = plan.table_tables_max;

    // Existing state of the target table (and the tenant's store) before any write.
    let path = tables::tenant_db_path(state, tenant.id);
    let name_c = a.name.clone();
    let mode_c = a.mode.clone();
    let (existing_schema, existing_rows, table_count, file_bytes) =
        tables::with_tenant_conn(path.clone(), state.db.cfg(), state.db.counters_handle(), move |conn| {
            let tables_n: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM {META_TABLE}"), [], |r| r.get(0))?;
            let schema = match tables::load_schema_sync(conn, &name_c) {
                Ok(s) => Some(s.columns),
                Err(e) if e.code() == "table_not_found" => None,
                Err(e) => return Err(e),
            };
            let rows = if schema.is_some() && mode_c == "append" {
                tables::row_count_sync(conn, &name_c)?
            } else {
                0
            };
            let fb = conn
                .path()
                .and_then(|p| std::fs::metadata(p).ok())
                .map(|m| m.len() as i64)
                .unwrap_or(0);
            Ok((schema, rows, tables_n, fb))
        })
        .await?;

    match (a.mode.as_str(), &existing_schema) {
        ("create", Some(_)) => {
            return Err(AppError::Structured {
                code: "table_already_exists",
                message: format!("table '{}' already exists (use mode: replace or append)", a.name),
                data: json!({"table": a.name}),
            })
        }
        ("append", None) => return Err(tables::table_not_found(&a.name)),
        _ => {}
    }

    // R4: counted from the record scan, before a single cell is converted.
    if existing_rows + found as i64 > rows_max {
        return Err(quota_refusal("quota_table_rows", "table_rows_max", rows_max, existing_rows, found as i64));
    }
    if existing_schema.is_none() && table_count >= tables_max {
        return Err(quota_refusal("quota_table_tables", "table_tables_max", tables_max, table_count, 1));
    }
    let est_bytes: i64 = records.iter().map(|r| r.1.iter().map(|c| c.len() as i64 + 1).sum::<i64>()).sum();
    if file_bytes + est_bytes > bytes_max {
        return Err(quota_refusal("quota_table_bytes", "table_bytes_max", bytes_max, file_bytes, est_bytes));
    }

    // Column set + types: append reuses the table's; otherwise infer over the sample.
    let (names, source_names, types): (Vec<String>, Vec<String>, Vec<CsvType>) = if a.mode == "append" {
        let schema = existing_schema.as_ref().expect("append checked above");
        let mut ts = Vec::new();
        for n in &names {
            let t = schema.get(n).ok_or_else(|| {
                structured(
                    "csv_columns_mismatch",
                    format!("column '{n}' is not in table '{}' (append never alters a table)", a.name),
                    json!({"column": n, "table": a.name, "columns_added": []}),
                )
            })?;
            ts.push(CsvType::from_column_type(*t));
        }
        (names, source_names, ts)
    } else {
        let sample: Vec<&(usize, Vec<String>)> = records.iter().take(infer_rows).collect();
        let mut ts = Vec::new();
        for (i, n) in names.iter().enumerate() {
            let forced = overrides.get(n).or_else(|| overrides.get(&source_names[i]));
            let t = match forced {
                Some(t) => *t,
                None => {
                    let cells: Vec<&str> = sample.iter().map(|r| r.1.get(i).map(String::as_str).unwrap_or("")).collect();
                    infer_column(&cells)
                }
            };
            ts.push(t);
        }
        (names, source_names, ts)
    };

    // Convert every row; a bad cell skips the row (or aborts the load).
    let mut loaded = Loaded {
        names: names.clone(),
        source_names: source_names.clone(),
        types: types.clone(),
        rows: Vec::with_capacity(records.len()),
        nulls: vec![0; names.len()],
        distinct: vec![BTreeSet::new(); names.len()],
        samples: vec![Vec::new(); names.len()],
        skipped: Vec::new(),
        rows_skipped: 0,
        found,
    };
    for (line, cells) in &records {
        let mut skip_reason: Option<(String, String)> = None;
        if cells.len() > names.len() {
            skip_reason = Some((String::new(), format!("expected {} fields, found {}", names.len(), cells.len())));
        }
        let mut row = Map::new();
        if skip_reason.is_none() {
            for (i, n) in names.iter().enumerate() {
                let raw = cells.get(i).map(String::as_str).unwrap_or("");
                match convert_cell(types[i], raw) {
                    Ok(v) => {
                        row.insert(n.clone(), v);
                    }
                    Err(reason) => {
                        skip_reason = Some((n.clone(), reason));
                        break;
                    }
                }
            }
        }
        if let Some((column, reason)) = skip_reason {
            if abort {
                return Err(structured(
                    "csv_row_invalid",
                    format!("row {line}{}: {reason}; nothing was loaded (on_error: abort)", col_suffix(&column)),
                    json!({"row": line, "column": col_value(&column), "reason": reason}),
                ));
            }
            loaded.rows_skipped += 1;
            if loaded.skipped.len() < SKIPPED_CAP {
                loaded.skipped.push(json!({"row": line, "column": col_value(&column), "reason": reason}));
            }
            continue;
        }
        for (i, n) in names.iter().enumerate() {
            match row.get(n) {
                Some(Value::Null) | None => loaded.nulls[i] += 1,
                Some(v) => {
                    if loaded.distinct[i].len() < 1000 {
                        loaded.distinct[i].insert(v.to_string());
                    }
                    if loaded.samples[i].len() < SAMPLES_PER_COLUMN && !loaded.samples[i].contains(v) {
                        loaded.samples[i].push(v.clone());
                    }
                }
            }
        }
        loaded.rows.push(row);
    }

    let columns_json: Vec<Value> = (0..loaded.names.len())
        .map(|i| {
            let mut c = json!({
                "name": loaded.names[i],
                "type": loaded.types[i].name(),
                "nulls": loaded.nulls[i],
                "distinct": loaded.distinct[i].len(),
                "samples": loaded.samples[i],
            });
            if header_rec.is_some() && loaded.source_names[i] != loaded.names[i] {
                c["source_name"] = json!(loaded.source_names[i]);
            }
            c
        })
        .collect();
    let preview: Vec<Value> = loaded.rows.iter().take(PREVIEW_ROWS).map(|r| Value::Object(r.clone())).collect();
    let sample_query = format!("SELECT * FROM {} LIMIT {PREVIEW_ROWS}", a.name);
    let mut out = json!({
        "table": a.name,
        "mode": a.mode,
        "rows_loaded": loaded.rows.len(),
        "rows_skipped": loaded.rows_skipped,
        "rows_found": loaded.found,
        "delimiter": delim.to_string(),
        "encoding": encoding,
        "columns": columns_json,
        "preview": preview,
        "skipped": loaded.skipped,
        "sample_query": sample_query,
    });

    let no_rows_err = || {
        structured(
            "csv_no_rows",
            format!("{which} '{key}' has a header but no data rows"),
            json!({which: key, "table": a.name, "columns": names, "rows_loaded": 0}),
        )
    };
    if a.dry_run {
        if found == 0 {
            return Err(no_rows_err());
        }
        out["dry_run"] = json!(true);
        out["created"] = json!(false);
        return Ok(out);
    }

    // One transaction: (drop,) create, insert. A failure leaves nothing behind.
    let schema_pairs: Vec<(String, ColumnType)> =
        loaded.names.iter().zip(loaded.types.iter()).map(|(n, t)| (n.clone(), t.column_type())).collect();
    let schema_json = serde_json::to_string(&Value::Object(
        schema_pairs.iter().map(|(c, t)| (c.clone(), json!(t.as_str()))).collect(),
    ))
    .map_err(|e| AppError::Internal(format!("schema serialize: {e}")))?;
    let rows = loaded.rows;
    let mode = a.mode.clone();
    let name = a.name.clone();
    let catalog = state.plans.clone();
    let billing_mode = state.billing_config.billing_mode();
    let plan_name = tenant.plan.clone();
    let table_rows_loaded = rows.len();
    tables::with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
        let nested = !conn.is_autocommit();
        if nested {
            conn.execute("SAVEPOINT csvtab_load", [])?;
        } else {
            conn.execute("BEGIN IMMEDIATE", [])?;
        }
        let outcome: Result<(), AppError> = (|| {
            // Re-check under the write lock: a concurrent load cannot slip past the quota.
            let used: i64 = if mode == "append" { tables::row_count_sync(conn, &name)? } else { 0 };
            if used + table_rows_loaded as i64 > rows_max {
                return Err(crate::billing::quota_exceeded_for(
                    &catalog, billing_mode, &plan_name, "table_rows_max", rows_max, used, None,
                ));
            }
            if mode == "replace" {
                let exists: i64 = conn.query_row(
                    &format!("SELECT COUNT(*) FROM {META_TABLE} WHERE name = ?1"),
                    params![name],
                    |r| r.get(0),
                )?;
                if exists > 0 {
                    conn.execute(&format!("DROP TABLE {}", tables::quote_ident(&name)), [])?;
                    conn.execute(&format!("DELETE FROM {META_TABLE} WHERE name = ?1"), params![name])?;
                }
            }
            if mode != "append" {
                let defs: Vec<String> = schema_pairs
                    .iter()
                    .map(|(c, t)| format!("{} {}", tables::quote_ident(c), t.sql_type()))
                    .collect();
                conn.execute(&format!("CREATE TABLE {} ({})", tables::quote_ident(&name), defs.join(", ")), [])?;
                conn.execute(
                    &format!(
                        "INSERT INTO {META_TABLE} (name, schema_json, primary_key, created_unix) VALUES (?1, ?2, NULL, ?3)"
                    ),
                    params![name, schema_json, crate::state::now_unix()],
                )?;
            }
            if !rows.is_empty() {
                let schema = tables::load_schema_sync(conn, &name)?;
                tables::insert_rows_sync(conn, &name, &schema, &rows)?;
            }
            Ok(())
        })();
        match outcome {
            Ok(()) => {
                if nested {
                    conn.execute("RELEASE csvtab_load", [])?;
                } else {
                    conn.execute("COMMIT", [])?;
                }
                Ok(())
            }
            Err(e) => {
                if nested {
                    let _ = conn.execute("ROLLBACK TO csvtab_load", []);
                    let _ = conn.execute("RELEASE csvtab_load", []);
                } else {
                    let _ = conn.execute("ROLLBACK", []);
                }
                Err(e)
            }
        }
    })
    .await?;

    if a.mode == "replace" {
        let _ = state.db.delete_table_model_and_annotations(tenant.id, a.name.clone()).await;
    }
    if let Err(e) = state.db.mark_table_model_stale(tenant.id, a.name.clone()).await {
        tracing::warn!(error = %e, table = %a.name, "failed to mark table model stale after from_blob");
    }
    if let Err(e) = state.db.mark_table_graph_stale(tenant.id).await {
        tracing::warn!(error = %e, table = %a.name, "failed to mark table graph stale after from_blob");
    }
    if found == 0 {
        // The (empty) table exists now; the refusal says so.
        return Err(no_rows_err());
    }
    out["created"] = json!(a.mode != "append");
    if a.mode == "append" {
        out["columns_added"] = json!([]);
    }
    Ok(out)
}

fn col_value(column: &str) -> Value {
    if column.is_empty() { Value::Null } else { json!(column) }
}

fn col_suffix(column: &str) -> String {
    if column.is_empty() { String::new() } else { format!(" column '{column}'") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arg_specs_match_the_struct_fields() {
        let args = TableFromBlobArgs {
            blob: None,
            doc: None,
            name: String::new(),
            header: true,
            delimiter: None,
            types: None,
            mode: String::new(),
            on_error: String::new(),
            infer_rows: None,
            dry_run: false,
            tenant_key: None,
        };
        let v = serde_json::to_value(&args).expect("serialize");
        let mut fields: Vec<&str> = v.as_object().expect("object").keys().map(String::as_str).collect();
        fields.retain(|f| *f != "tenant_key");
        fields.sort_unstable();
        let mut specs: Vec<&str> = ARG_SPECS.iter().map(|a| a.name).collect();
        specs.sort_unstable();
        assert_eq!(fields, specs, "csvtab::ARG_SPECS drifted from TableFromBlobArgs");
    }

    #[test]
    fn delimiter_and_quotes() {
        assert_eq!(detect_delimiter("a;b;c\n1;2;3\n"), ';');
        assert_eq!(detect_delimiter("a\tb\n1\t2\n"), '\t');
        assert_eq!(detect_delimiter("a,b\n\"x;y;z\",2\n"), ',');
        let recs = parse_records("a,b\n\"x\ny\",2\n\n3,4\n", ',');
        assert_eq!(recs.len(), 3);
        assert_eq!(recs[1], (2, vec!["x\ny".to_string(), "2".to_string()]));
        assert_eq!(recs[2].0, 5, "line numbers count newlines inside quotes");
    }

    #[test]
    fn inference_rules() {
        assert_eq!(infer_column(&["1", "2", ""]), CsvType::Integer);
        assert_eq!(infer_column(&["1", "2.5"]), CsvType::Real);
        assert_eq!(infer_column(&["yes", "No", "1"]), CsvType::Bool);
        assert_eq!(infer_column(&["0", "1"]), CsvType::Integer);
        assert_eq!(infer_column(&["2024-01-02", "2024-01-02T03:04:05Z"]), CsvType::Date);
        assert_eq!(infer_column(&["00123", "9"]), CsvType::Text, "a leading zero vetoes integer");
        assert_eq!(infer_column(&["", ""]), CsvType::Text);
        assert_eq!(infer_column(&["1", "n/a"]), CsvType::Text);
        assert_eq!(infer_column(&["nan", "inf"]), CsvType::Text);
        let mut many: Vec<String> = (0..300).map(|i| format!("{i}.5")).collect();
        many[7] = "n/a".to_string();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        assert_eq!(infer_column(&refs), CsvType::Real, "one stray in 300 keeps the column real");
        let mut few: Vec<String> = (0..50).map(|i| format!("{i}.5")).collect();
        few[7] = "n/a".to_string();
        let refs: Vec<&str> = few.iter().map(String::as_str).collect();
        assert_eq!(infer_column(&refs), CsvType::Text, "no tolerance below 100 cells");
    }

    #[test]
    fn encodings() {
        assert_eq!(decode_text(b"\xEF\xBB\xBFa,b").0, "a,b");
        let mut u16le = vec![0xFF, 0xFE];
        for u in "a,b\n1,2\n".encode_utf16() {
            u16le.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(decode_text(&u16le).0, "a,b\n1,2\n");
        assert_eq!(decode_text(b"caf\xE9").0, "café");
    }

    #[test]
    fn header_sanitising() {
        let mut taken = BTreeSet::new();
        assert_eq!(sanitise_header("Order Date", 0, &taken), "Order_Date");
        taken.insert("a".to_string());
        assert_eq!(sanitise_header("a", 1, &taken), "a_2");
        assert_eq!(sanitise_header("", 2, &taken), "col3");
        assert_eq!(sanitise_header("1st", 3, &taken), "c_1st");
    }

    #[test]
    fn recipe_splice_is_idempotent() {
        let doc = format!("x\n{RECIPE_START}\nstale\n{RECIPE_END}\ny\n");
        let once = splice_recipe_section(&doc);
        assert_eq!(splice_recipe_section(&once), once);
        assert!(once.contains("`from_blob`") || once.contains("host.table.from_blob"));
    }
}
