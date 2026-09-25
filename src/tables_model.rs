//! PRD-mcphost-table-semantic-model: `host.table.describe` and friends -- a
//! generated semantic model over a tenant's own `host.table.*` tables
//! (`tables.rs`), so an agent writing a tool over a table it just loaded
//! learns the column types, keys, categories, measures and dates without
//! sampling rows itself (Problem statement: "an agent writing a tool over
//! it must sample rows to learn types, guess keys, and hard-code
//! measures").
//!
//! Inference runs over a sample of up to [`SAMPLE_CAP`] rows of the
//! tenant's own per-tenant SQLite table (requirement 2), entirely in Rust
//! over the sampled values -- not by SQLite `typeof()` alone, since every
//! `host.table.*` column is already declared a real [`ColumnType`] at
//! `host.table.create` time (unlike the CSV-derived TEXT columns the
//! PRD's own technical-considerations note anticipated).

use std::collections::{BTreeMap, HashMap, HashSet};

use rusqlite::Connection;
use serde_json::{Map, Value, json};

use crate::db::{TableModelAnnotationRow, Tenant};
use crate::errors::AppError;
use crate::state::AppState;
use crate::tables::{self, ColumnType};

/// requirement 2/6: inference samples at most this many rows (all rows
/// when the table has fewer) -- AC6's bound.
pub const SAMPLE_CAP: i64 = 10_000;

/// requirement 5: the background tick's cadence -- see [`spawn_tick`].
const TICK_INTERVAL_SECS: u64 = 10;

/// requirement 2: `key` when every sampled value is distinct and none are
/// null; `category` when at most this many distinct values and under 5%
/// distinct-share; `id` when text with over 90% distinct-share and a
/// stable length; `date` when at least this fraction of non-null values
/// parse as an ISO date.
const CATEGORY_MAX_DISTINCT: i64 = 20;
const CATEGORY_MAX_DISTINCT_SHARE: f64 = 0.05;
const ID_MIN_DISTINCT_SHARE: f64 = 0.9;
const DATE_MIN_PARSE_RATE: f64 = 0.95;

fn round4(x: f64) -> f64 {
    (x * 10_000.0).round() / 10_000.0
}

/// A loose "looks like an ISO date (or datetime)" check: a `YYYY-MM-DD`
/// prefix with an in-range month/day, optionally followed by a time part.
/// Deliberately not a full RFC 3339 parser -- this is a heuristic over
/// sampled values (requirement 2's `date` rule), not a validator, and this
/// crate already avoids a `chrono`/`time` dependency for small,
/// self-contained date math (see `state::rfc3339_from_unix`).
fn is_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() < 10 {
        return false;
    }
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    if !(digits(0..4) && b[4] == b'-' && digits(5..7) && b[7] == b'-' && digits(8..10)) {
        return false;
    }
    let month: u32 = s[5..7].parse().unwrap_or(0);
    let day: u32 = s[8..10].parse().unwrap_or(0);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return false;
    }
    b.len() == 10 || b[10] == b'T' || b[10] == b' '
}

fn canonical_key(v: &Value) -> String {
    match v {
        Value::String(s) => format!("s:{s}"),
        Value::Bool(b) => format!("b:{b}"),
        Value::Number(n) => format!("n:{n}"),
        other => other.to_string(),
    }
}

/// One column's analysis over the sample -- both the reported fields
/// (requirement 2) and [`ColumnAnalysis::value_keys`], the distinct
/// non-null value set requirement 3's foreign-key detection checks for
/// containment against a candidate target table's own key column.
struct ColumnAnalysis {
    type_: &'static str,
    role: &'static str,
    null_share: f64,
    distinct: i64,
    distinct_share: f64,
    min: Value,
    max: Value,
    top_values: Vec<Value>,
    value_keys: HashSet<String>,
}

impl ColumnAnalysis {
    /// AC10: an empty table has no sample to analyze -- every column
    /// reports `type: "unknown"` rather than guessing from the declared
    /// (but never-populated) schema type.
    fn empty_unknown() -> Self {
        ColumnAnalysis {
            type_: "unknown",
            role: "text",
            null_share: 0.0,
            distinct: 0,
            distinct_share: 0.0,
            min: Value::Null,
            max: Value::Null,
            top_values: Vec::new(),
            value_keys: HashSet::new(),
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "type": self.type_,
            "role": self.role,
            "inferred_role": self.role,
            "null_share": round4(self.null_share),
            "distinct": self.distinct,
            "distinct_share": round4(self.distinct_share),
            "min": self.min,
            "max": self.max,
            "top_values": self.top_values,
        })
    }
}

/// Accumulates one column's sampled values before [`ColBuilder::finish`]
/// turns the accumulation into a [`ColumnAnalysis`] -- kept as a separate
/// type so the running state (frequency table, per-value example, running
/// min/max) doesn't leak into the reported shape above.
#[derive(Default)]
struct ColBuilder {
    null_count: i64,
    non_null: i64,
    freq: HashMap<String, i64>,
    example_by_key: HashMap<String, Value>,
    lengths: HashSet<usize>,
    numeric_min: Option<f64>,
    numeric_max: Option<f64>,
    text_min: Option<String>,
    text_max: Option<String>,
    date_parseable: i64,
}

impl ColBuilder {
    fn push(&mut self, v: Value) {
        if v.is_null() {
            self.null_count += 1;
            return;
        }
        self.non_null += 1;
        let key = canonical_key(&v);
        *self.freq.entry(key.clone()).or_insert(0) += 1;
        self.example_by_key.entry(key).or_insert_with(|| v.clone());
        if let Value::String(s) = &v {
            self.lengths.insert(s.chars().count());
            if is_iso_date(s) {
                self.date_parseable += 1;
            }
            if self.text_min.as_deref().is_none_or(|cur| s.as_str() < cur) {
                self.text_min = Some(s.clone());
            }
            if self.text_max.as_deref().is_none_or(|cur| s.as_str() > cur) {
                self.text_max = Some(s.clone());
            }
        }
        if let Some(n) = v.as_f64() {
            self.numeric_min = Some(self.numeric_min.map_or(n, |m| m.min(n)));
            self.numeric_max = Some(self.numeric_max.map_or(n, |m| m.max(n)));
        }
    }

    /// requirement 2's five rules, applied in the order the requirement
    /// states them (`key`, `category`, `measure`, `date`, `id`) -- the
    /// first that matches wins, `text` otherwise.
    fn finish(self, ty: ColumnType, sample_count: i64) -> ColumnAnalysis {
        let distinct = self.freq.len() as i64;
        let null_share = if sample_count > 0 {
            self.null_count as f64 / sample_count as f64
        } else {
            0.0
        };
        let distinct_share = if sample_count > 0 {
            distinct as f64 / sample_count as f64
        } else {
            0.0
        };
        let is_numeric = matches!(ty, ColumnType::Integer | ColumnType::Real);
        let is_key = self.null_count == 0 && distinct == sample_count && sample_count > 0;
        let is_category =
            distinct <= CATEGORY_MAX_DISTINCT && distinct_share < CATEGORY_MAX_DISTINCT_SHARE && sample_count > 0;
        let is_measure = is_numeric && !is_key;
        let date_rate = if self.non_null > 0 {
            self.date_parseable as f64 / self.non_null as f64
        } else {
            0.0
        };
        let is_date = self.non_null > 0 && date_rate >= DATE_MIN_PARSE_RATE;
        let stable_length = self.lengths.len() == 1;
        let is_id = matches!(ty, ColumnType::Text) && distinct_share > ID_MIN_DISTINCT_SHARE && stable_length;

        let role = if is_key {
            "key"
        } else if is_category {
            "category"
        } else if is_measure {
            "measure"
        } else if is_date {
            "date"
        } else if is_id {
            "id"
        } else {
            "text"
        };

        let type_ = match ty {
            ColumnType::Text => "text",
            ColumnType::Integer => "integer",
            ColumnType::Real => "number",
            ColumnType::Boolean => "boolean",
            ColumnType::Timestamp => "datetime",
            ColumnType::Json => "text",
        };

        let (min, max) = if is_numeric {
            let to_json = |n: f64| -> Value {
                if matches!(ty, ColumnType::Integer) { json!(n as i64) } else { json!(n) }
            };
            (
                self.numeric_min.map(to_json).unwrap_or(Value::Null),
                self.numeric_max.map(to_json).unwrap_or(Value::Null),
            )
        } else if matches!(ty, ColumnType::Timestamp) || is_date {
            (
                self.text_min.clone().map(Value::String).unwrap_or(Value::Null),
                self.text_max.clone().map(Value::String).unwrap_or(Value::Null),
            )
        } else {
            (Value::Null, Value::Null)
        };

        let mut counts: Vec<(String, i64)> = self.freq.iter().map(|(k, v)| (k.clone(), *v)).collect();
        counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let top_values: Vec<Value> = counts
            .into_iter()
            .take(5)
            .map(|(key, count)| {
                let value = self.example_by_key.get(&key).cloned().unwrap_or(Value::Null);
                json!({"value": value, "count": count})
            })
            .collect();

        let value_keys: HashSet<String> = self.freq.into_keys().collect();

        ColumnAnalysis {
            type_,
            role,
            null_share,
            distinct,
            distinct_share,
            min,
            max,
            top_values,
            value_keys,
        }
    }
}

/// [`analyze_table_columns`]'s return shape: `(row_count, sample_count,
/// sampled, per-column analysis)`. `row_count` is the table's true row
/// count (never capped); `sampled` is AC6's flag, `true` when `row_count`
/// exceeds [`SAMPLE_CAP`].
type TableAnalysis = (i64, i64, bool, BTreeMap<String, ColumnAnalysis>);

/// Samples up to [`SAMPLE_CAP`] rows of `table` and analyzes every declared
/// column.
fn analyze_table_columns(conn: &Connection, table: &str) -> Result<TableAnalysis, AppError> {
    let schema = tables::load_schema_sync(conn, table)?;
    let row_count = tables::row_count_sync(conn, table)?;
    if row_count == 0 {
        let columns = schema.columns.keys().map(|c| (c.clone(), ColumnAnalysis::empty_unknown())).collect();
        return Ok((0, 0, false, columns));
    }
    let sampled = row_count > SAMPLE_CAP;

    let col_names: Vec<String> = schema.columns.keys().cloned().collect();
    let select_cols = col_names.iter().map(|c| tables::quote_ident(c)).collect::<Vec<_>>().join(", ");
    let sql = format!(
        "SELECT {select_cols} FROM {} LIMIT {SAMPLE_CAP}",
        tables::quote_ident(table)
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query([])?;

    let mut builders: BTreeMap<String, ColBuilder> =
        col_names.iter().map(|c| (c.clone(), ColBuilder::default())).collect();
    let mut sample_count: i64 = 0;
    while let Some(row) = rows.next()? {
        sample_count += 1;
        for (i, name) in col_names.iter().enumerate() {
            let value = tables::value_ref_to_json(row.get_ref(i)?);
            builders.get_mut(name).expect("built from the same column list").push(value);
        }
    }

    let columns = builders
        .into_iter()
        .map(|(name, builder)| {
            let ty = schema.columns[&name];
            let analysis = builder.finish(ty, sample_count);
            (name, analysis)
        })
        .collect();

    Ok((row_count, sample_count, sampled, columns))
}

/// requirement 3's cap: foreign-key detection checks a candidate source
/// column against at most this many sibling tables.
const MAX_SIBLING_TABLES: usize = 50;

/// requirement 3: ≥ this fraction of a candidate column's own distinct
/// (sampled) values must appear in a sibling table's key column for the
/// pair to be reported as a foreign key.
const FK_MIN_COVERAGE: f64 = 0.95;

/// requirement 3: `pairs where ≥ 95% of a column's distinct values appear
/// in another table's key column of the same tenant, checked only for
/// columns named *_id or matching a key column name`. `columns` is the
/// source table's own analysis (already computed); every other declared
/// table in the tenant (bounded to [`MAX_SIBLING_TABLES`]) is sampled and
/// analyzed again here to find its own `key`-role column, the same rule
/// [`compute_model_sync`] uses for the source table's own `primary_key`.
fn detect_foreign_keys(conn: &Connection, table: &str, columns: &BTreeMap<String, ColumnAnalysis>) -> Vec<Value> {
    let Ok(all_tables) = tables::list_table_names_sync(conn) else {
        return Vec::new();
    };

    // (target table name, its key column name, that column's value set).
    let mut sibling_keys: Vec<(String, String, &HashSet<String>)> = Vec::new();
    let mut sibling_analyses: Vec<(String, BTreeMap<String, ColumnAnalysis>)> = Vec::new();
    for other in all_tables.iter().filter(|t| t.as_str() != table).take(MAX_SIBLING_TABLES) {
        if let Ok((_, _, _, other_columns)) = analyze_table_columns(conn, other) {
            sibling_analyses.push((other.clone(), other_columns));
        }
    }
    for (other, other_columns) in &sibling_analyses {
        if let Some((key_col, key_analysis)) = other_columns.iter().find(|(_, a)| a.role == "key") {
            sibling_keys.push((other.clone(), key_col.clone(), &key_analysis.value_keys));
        }
    }

    let key_names_elsewhere: HashSet<&str> = sibling_keys.iter().map(|(_, k, _)| k.as_str()).collect();

    let mut foreign_keys = Vec::new();
    for (col_name, analysis) in columns {
        let is_candidate = col_name.ends_with("_id") || key_names_elsewhere.contains(col_name.as_str());
        if !is_candidate || analysis.value_keys.is_empty() {
            continue;
        }
        for (other_table, key_col, key_values) in &sibling_keys {
            if key_values.is_empty() {
                continue;
            }
            let matched = analysis.value_keys.iter().filter(|v| key_values.contains(v.as_str())).count();
            let coverage = matched as f64 / analysis.value_keys.len() as f64;
            if coverage >= FK_MIN_COVERAGE {
                foreign_keys.push(json!({
                    "column": col_name,
                    "references_table": other_table,
                    "references_column": key_col,
                }));
            }
        }
    }
    foreign_keys
}

/// requirement 3: `sum`/`avg`/`min`/`max` suggested for every `measure`
/// column -- the PRD's own worked example ("`amount` is a measure, the
/// tool it writes is right the first time").
const MEASURE_SUGGESTIONS: &[&str] = &["sum", "avg", "min", "max"];

/// requirement 3: table-level `row_count`, `primary_key`, `foreign_keys`,
/// `measures`, `dimensions` (`category`/`date` columns), plus the full
/// per-column shape requirement 2 asks for.
fn compute_model_sync(conn: &Connection, table: &str) -> Result<Value, AppError> {
    let (row_count, sample_count, sampled, columns) = analyze_table_columns(conn, table)?;

    let primary_key = columns.iter().find(|(_, a)| a.role == "key").map(|(name, _)| name.clone());
    let foreign_keys = detect_foreign_keys(conn, table, &columns);
    let measures: Vec<Value> = columns
        .iter()
        .filter(|(_, a)| a.role == "measure")
        .map(|(name, _)| json!({"column": name, "suggestions": MEASURE_SUGGESTIONS}))
        .collect();
    let dimensions: Vec<&String> =
        columns.iter().filter(|(_, a)| a.role == "category" || a.role == "date").map(|(name, _)| name).collect();

    let columns_json: Map<String, Value> =
        columns.iter().map(|(name, analysis)| (name.clone(), analysis.to_json())).collect();

    Ok(json!({
        "row_count": row_count,
        "sample_count": sample_count,
        "sampled": sampled,
        "primary_key": primary_key,
        "foreign_keys": foreign_keys,
        "measures": measures,
        "dimensions": dimensions,
        "columns": Value::Object(columns_json),
    }))
}

/// requirement 6: the only keys `host.table.model_set` accepts.
const ANNOTATION_KEYS: &[&str] = &["role", "unit", "description", "hidden"];

/// requirement 4: "annotations merged (annotation wins over inference for
/// `role`, adds `unit`, `description`)". A table-level annotation
/// (`column_name == ""`, see the migration's own default) is set directly
/// on the model's top level; a column-level one is set on that column's
/// own object. `role` overwrites the column's `role` field but leaves
/// `inferred_role` (the raw classification) untouched -- AC8's own proof.
fn merge_annotations(mut model: Value, annotations: &[TableModelAnnotationRow]) -> Value {
    for ann in annotations {
        if ann.column_name.is_empty() {
            if let Some(obj) = model.as_object_mut() {
                let value = if ann.key == "hidden" { json!(ann.value == "true") } else { json!(ann.value) };
                obj.insert(ann.key.clone(), value);
            }
            continue;
        }
        let Some(col) = model.get_mut("columns").and_then(|c| c.get_mut(&ann.column_name)).and_then(Value::as_object_mut)
        else {
            continue;
        };
        let value = if ann.key == "hidden" { json!(ann.value == "true") } else { json!(ann.value) };
        col.insert(ann.key.clone(), value);
    }
    model
}

/// requirement 4: `host.table.describe {table}` -- the latest stored
/// model, bootstrapping a synchronous compute (version 1, not stale) the
/// first time a table is ever described, so AC1 sees a fresh model without
/// waiting on the background tick. A table with an existing (possibly
/// stale) model is returned as-is; `describe` never blocks on a recompute
/// (requirement 5). Every stored annotation (requirement 6) is merged in
/// last, so it always wins over the freshly (re)computed inference.
pub async fn table_describe(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let table = tables::arg_str(args, "table")?;

    let existing = state.db.get_table_model(tenant.id, table.clone()).await?;
    let (model, version, stale, computed_at) = match existing {
        Some(row) => {
            let model: Value = serde_json::from_str(&row.model_json)
                .map_err(|e| AppError::Internal(format!("stored table model_json is corrupt: {e}")))?;
            (model, row.version, row.stale, row.computed_at)
        }
        None => {
            let path = tables::tenant_db_path(state, tenant.id);
            let table_for_conn = table.clone();
            let model = tables::with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| compute_model_sync(conn, &table_for_conn)).await?;
            let row_count = model["row_count"].as_i64().unwrap_or(0);
            let model_json = serde_json::to_string(&model)
                .map_err(|e| AppError::Internal(format!("table model serialize: {e}")))?;
            state.db.upsert_table_model(tenant.id, table.clone(), 1, model_json, row_count).await?;
            (model, 1, false, crate::state::now_unix())
        }
    };

    let annotations = state.db.list_table_model_annotations(tenant.id, table.clone()).await?;
    let model = merge_annotations(model, &annotations);

    let mut out = model.as_object().cloned().unwrap_or_default();
    out.insert("table".to_string(), json!(table));
    out.insert("version".to_string(), json!(version));
    out.insert("stale".to_string(), json!(stale));
    out.insert("computed_at".to_string(), json!(computed_at));
    Ok(Value::Object(out))
}

/// requirement 6: `host.table.model_set {table, column?, key, value}` --
/// records an annotation that survives refresh (`describe` merges it back
/// in on every call, see [`merge_annotations`]). `column` is optional (a
/// table-level annotation); `key` must be one of [`ANNOTATION_KEYS`].
pub async fn model_set(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let table = tables::arg_str(args, "table")?;
    let column = tables::arg_str_opt(args, "column").unwrap_or_default();
    let key = tables::arg_str(args, "key")?;
    if !ANNOTATION_KEYS.contains(&key.as_str()) {
        return Err(AppError::InvalidArgs(format!(
            "key: must be one of role, unit, description, hidden; got '{key}'"
        )));
    }
    let value_arg = args.get("value").ok_or_else(|| AppError::InvalidArgs("missing required argument 'value'".to_string()))?;
    let value = match value_arg {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };

    // Verify the table is actually declared -- surfaces `table_not_found`
    // rather than silently recording an annotation for a table that will
    // never be described.
    let path = tables::tenant_db_path(state, tenant.id);
    let table_for_check = table.clone();
    tables::with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| tables::load_schema_sync(conn, &table_for_check).map(|_| ())).await?;

    state.db.upsert_table_model_annotation(tenant.id, table.clone(), column, key, value).await?;
    Ok(json!({"table": table, "set": true}))
}

/// requirement 4: `host.table.models` -- every table this tenant has a
/// stored model for, with its version/staleness/row_count, table name
/// ascending. AC9: a dropped table's row is gone (`table_drop` deletes it),
/// so this never lists a table that no longer exists. A table that has
/// never been `describe`d yet (no model row) simply doesn't appear --
/// `host.table.list` (`tables.rs`) is the exhaustive "every declared
/// table" listing; this one is "every table with a computed model".
pub async fn table_models_list(state: &AppState, tenant: &Tenant, _args: &Value) -> Result<Value, AppError> {
    let rows = state.db.list_table_models(tenant.id).await?;
    let tables: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            json!({
                "table": r.table_name,
                "version": r.version,
                "stale": r.stale,
                "row_count": r.row_count,
                "computed_at": r.computed_at,
            })
        })
        .collect();
    Ok(json!({"tables": tables}))
}

/// requirement 5: recomputes every stale model across every tenant,
/// bumping its version and clearing `stale`. Exposed directly (not just
/// via [`spawn_tick`]) so a test can drive one deterministic cycle instead
/// of waiting on the real cadence -- same convention as
/// `bans::tick_once`/`triggers::tick_once`. A recompute failure for one
/// (tenant, table) is logged and skipped, not propagated -- one broken
/// table's model must never block every other tenant's tick.
pub async fn tick_once(state: &AppState) -> Result<(), AppError> {
    let stale = state.db.list_stale_table_models().await?;
    for (tenant_id, table, prev_version) in stale {
        let path = tables::tenant_db_path(state, tenant_id);
        let table_for_conn = table.clone();
        let model = match tables::with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| compute_model_sync(conn, &table_for_conn)).await
        {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(error = %e, tenant_id, table = %table, "table model recompute failed");
                continue;
            }
        };
        let row_count = model["row_count"].as_i64().unwrap_or(0);
        let model_json = match serde_json::to_string(&model) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, tenant_id, table = %table, "table model serialize failed");
                continue;
            }
        };
        if let Err(e) = state
            .db
            .upsert_table_model(tenant_id, table.clone(), prev_version + 1, model_json, row_count)
            .await
        {
            tracing::warn!(error = %e, tenant_id, table = %table, "table model store failed");
        }
    }
    Ok(())
}

/// requirement 5: "the cron loop recomputes stale models within 30s, at
/// most once per 10s per table" -- a 10s tick cadence bounds both clauses
/// at once (a stale row is picked up on the very next tick, well under
/// 30s, and can be recomputed at most once per 10s since that's the
/// entire loop's own period), so [`tick_once`] itself needs no per-row
/// cooldown bookkeeping. Started once alongside this crate's other
/// background tasks (see `main.rs`), same spawn/sleep-loop shape as
/// `bans::spawn_tick`.
pub fn spawn_tick(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(TICK_INTERVAL_SECS)).await;
            if let Err(e) = tick_once(&state).await {
                tracing::warn!(error = %e, "table model tick failed");
            }
        }
    })
}
