//! Business logic for `host.state.*` (PRD-mcphost-tenant-state P0
//! requirements 1-4): a per-tenant key-value namespace plus tables with a
//! declared, validated schema. Pure `AppState` + arguments in,
//! `serde_json::Value` (or [`AppError`]) out, same convention as
//! `control.rs` -- `handler.rs` is the only place that touches `rmcp` wire
//! types.
//!
//! Storage lives in `db.rs`'s `tenant_state_kv`/`tenant_state_tables`/
//! `tenant_state_rows` (migration 0011); this module owns everything
//! `db.rs` deliberately doesn't: column-type validation, the small filter
//! grammar `host.state.query`/`delete_rows` share, and quota arithmetic
//! against `plans.toml`'s `state_bytes_max`/`state_rows_max`/
//! `state_ops_per_call_max`.
//!
//! Requirement 3 (a `mcphost.state` module inside the python sandbox) and
//! requirement 5 (state counters on `host.tool_test`/`host.tool_logs`/
//! `host.usage`) are not implemented by this increment -- see the PRD's
//! open question on the sandbox channel; this module is the ground they
//! stand on (the same P0 storage + control-plane surface the sandbox
//! bridge will call into once it exists).

use serde_json::{Map, Value, json};

use crate::db::{Db, Tenant};
use crate::errors::AppError;
use crate::plans::Plan;
use crate::state::AppState;

/// requirement 1: "typed columns (text, integer, real, boolean, json)".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnType {
    Text,
    Integer,
    Real,
    Boolean,
    Json,
}

impl ColumnType {
    pub fn as_str(self) -> &'static str {
        match self {
            ColumnType::Text => "text",
            ColumnType::Integer => "integer",
            ColumnType::Real => "real",
            ColumnType::Boolean => "boolean",
            ColumnType::Json => "json",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "text" => Some(ColumnType::Text),
            "integer" => Some(ColumnType::Integer),
            "real" => Some(ColumnType::Real),
            "boolean" => Some(ColumnType::Boolean),
            "json" => Some(ColumnType::Json),
            _ => None,
        }
    }

    /// `true` if `value`'s JSON shape matches this declared type. `Real`
    /// accepts an integer literal too (JSON has one number type; `3` is a
    /// valid `real`), the same permissiveness `json_extract`-style typed
    /// stores generally give numeric columns.
    fn matches(self, value: &Value) -> bool {
        match self {
            ColumnType::Text => value.is_string(),
            ColumnType::Integer => value.is_i64() || value.is_u64(),
            ColumnType::Real => value.is_number(),
            ColumnType::Boolean => value.is_boolean(),
            ColumnType::Json => true,
        }
    }
}

/// A parsed `schema_json` -- ordered isn't required, so a plain map is
/// enough. Built once per `table_create`/`insert`/`query` call from the
/// stored `schema_json` text.
pub struct TableSchema {
    pub columns: std::collections::BTreeMap<String, ColumnType>,
    pub primary_key: Option<String>,
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

/// requirement 4: `state_quota_exceeded`, naming the quota and the current
/// value (AC6). Distinct from `billing::quota_exceeded`'s generic
/// `quota_exceeded` code/`limit` field shape -- this PRD's AC6 pins the
/// wire code to exactly `state_quota_exceeded` and the field name to
/// `quota`.
fn quota_exceeded(quota: &'static str, limit: i64, used: i64) -> AppError {
    AppError::Structured {
        code: "state_quota_exceeded",
        message: format!("state quota exceeded: {quota} (limit {limit}, at {used})"),
        data: json!({"quota": quota, "limit": limit, "used": used}),
    }
}

/// requirement 1/AC5: `state_schema_violation`, naming the failing column
/// and the type it was declared as. `data` uses the shared `field`/
/// `expected` key names every other structured error in this crate does
/// (see `errors::AppError::field_and_expected`'s `Structured` arm) so
/// `into_error_data` surfaces them at the top level unclobbered, instead of
/// falling back to `split_field`'s whole-message guess.
fn schema_violation(field: &str, expected: &str) -> AppError {
    AppError::Structured {
        code: "state_schema_violation",
        message: format!("column '{field}' must be of type '{expected}'"),
        data: json!({"field": field, "expected": expected}),
    }
}

fn table_not_found(table: &str) -> AppError {
    AppError::Structured {
        code: "state_table_not_found",
        message: format!("no table '{table}' is declared for this tenant"),
        data: json!({"table": table}),
    }
}

fn plan_of<'a>(state: &'a AppState, plan_name: &str) -> Result<&'a Plan, AppError> {
    state.plans.get(plan_name).ok_or_else(|| {
        AppError::Internal(format!(
            "tenant's plan '{plan_name}' is not in the loaded plan catalog"
        ))
    })
}

/// requirement 4's byte-quota check, shared by every write path (`kv_set`,
/// `insert`, `import`). `additional_bytes` is the size of what's about to
/// be written; the check runs *before* the write, so a rejected call
/// leaves the store unchanged (AC6: "nothing is written" for the schema
/// case reads the same way for the quota case).
async fn check_bytes_quota(
    db: &Db,
    tenant_id: i64,
    plan: &Plan,
    additional_bytes: i64,
) -> Result<(), AppError> {
    let used = db.state_bytes_used(tenant_id).await?;
    if used + additional_bytes > plan.state_bytes_max {
        return Err(quota_exceeded("state_bytes_max", plan.state_bytes_max, used));
    }
    Ok(())
}

fn parse_schema(schema_json: &str) -> Result<TableSchema, AppError> {
    let raw: Value = serde_json::from_str(schema_json)
        .map_err(|e| AppError::Internal(format!("stored schema_json is corrupt: {e}")))?;
    let obj = raw
        .as_object()
        .ok_or_else(|| AppError::Internal("stored schema_json is not an object".to_string()))?;
    let mut columns = std::collections::BTreeMap::new();
    for (k, v) in obj {
        let ty = v
            .as_str()
            .and_then(ColumnType::parse)
            .ok_or_else(|| AppError::Internal(format!("stored schema_json has a bad type for column '{k}'")))?;
        columns.insert(k.clone(), ty);
    }
    // primary_key is stored as its own DB column, applied by the caller.
    Ok(TableSchema {
        columns,
        primary_key: None,
    })
}

// ---- host.state.get/set/delete/list ------------------------------------

pub async fn state_get(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let key = arg_str(args, "key")?;
    match state.db.state_kv_get(tenant.id, key.clone()).await? {
        Some((value_json, updated_unix)) => {
            let value: Value = serde_json::from_str(&value_json)
                .map_err(|e| AppError::Internal(format!("stored value_json is corrupt: {e}")))?;
            Ok(json!({"key": key, "value": value, "updated_unix": updated_unix, "found": true}))
        }
        None => Ok(json!({"key": key, "value": Value::Null, "found": false})),
    }
}

pub async fn state_set(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let key = arg_str(args, "key")?;
    let value = args
        .get("value")
        .cloned()
        .ok_or_else(|| AppError::InvalidArgs("missing required argument 'value'".to_string()))?;
    let value_json = serde_json::to_string(&value)
        .map_err(|e| AppError::Internal(format!("value serialize: {e}")))?;

    let plan = plan_of(state, &tenant.plan)?;

    // A re-set of an existing key must not double-count its own prior
    // bytes against the quota (same "already_exists" carve-out
    // `control::secret_set` uses for `secrets_max`).
    let existing_bytes = state
        .db
        .state_kv_get(tenant.id, key.clone())
        .await?
        .map(|(v, _)| v.len() as i64)
        .unwrap_or(0);
    check_bytes_quota(
        &state.db,
        tenant.id,
        plan,
        value_json.len() as i64 - existing_bytes,
    )
    .await?;

    state.db.state_kv_set(tenant.id, key.clone(), value_json).await?;
    Ok(json!({"key": key, "set": true}))
}

pub async fn state_delete(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let key = arg_str(args, "key")?;
    let deleted = state.db.state_kv_delete(tenant.id, key.clone()).await?;
    Ok(json!({"key": key, "deleted": deleted}))
}

pub async fn state_list(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let prefix = arg_str_opt(args, "prefix");
    let limit = arg_i64_opt(args, "limit").unwrap_or(100).clamp(1, 1000);
    let rows = state.db.state_kv_list(tenant.id, prefix, limit).await?;
    let keys: Vec<Value> = rows
        .into_iter()
        .map(|(key, value_json, updated_unix)| {
            let value: Value = serde_json::from_str(&value_json).unwrap_or(Value::Null);
            json!({"key": key, "value": value, "updated_unix": updated_unix})
        })
        .collect();
    Ok(json!({"keys": keys}))
}

// ---- host.state.table_create/table_drop ---------------------------------

/// requirement 2: `schema` is `{"column": "text"|"integer"|"real"|
/// "boolean"|"json", ...}`; `primary_key`, if given, must name one of
/// `schema`'s own columns.
pub async fn state_table_create(
    state: &AppState,
    tenant: &Tenant,
    args: &Value,
) -> Result<Value, AppError> {
    let name = arg_str(args, "name")?;
    let schema_val = args
        .get("schema")
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| AppError::InvalidArgs("missing required argument 'schema'".to_string()))?;
    if schema_val.is_empty() {
        return Err(AppError::InvalidArgs("'schema' must declare at least one column".to_string()));
    }
    let mut validated = Map::new();
    for (col, ty) in &schema_val {
        let ty_str = ty
            .as_str()
            .ok_or_else(|| AppError::InvalidArgs(format!("column '{col}' type must be a string")))?;
        if ColumnType::parse(ty_str).is_none() {
            return Err(AppError::InvalidArgs(format!(
                "column '{col}' has unknown type '{ty_str}'; expected one of \
                 text, integer, real, boolean, json"
            )));
        }
        validated.insert(col.clone(), ty.clone());
    }
    let primary_key = arg_str_opt(args, "primary_key");
    if let Some(pk) = &primary_key
        && !validated.contains_key(pk)
    {
        return Err(AppError::InvalidArgs(format!(
            "primary_key '{pk}' is not one of the declared columns"
        )));
    }
    let schema_json = serde_json::to_string(&Value::Object(validated))
        .map_err(|e| AppError::Internal(format!("schema serialize: {e}")))?;
    state
        .db
        .state_table_create(tenant.id, name.clone(), schema_json, primary_key)
        .await?;
    Ok(json!({"name": name, "created": true}))
}

pub async fn state_table_drop(
    state: &AppState,
    tenant: &Tenant,
    args: &Value,
) -> Result<Value, AppError> {
    let name = arg_str(args, "name")?;
    let dropped = state.db.state_table_drop(tenant.id, name.clone()).await?;
    Ok(json!({"name": name, "dropped": dropped}))
}

async fn load_table(
    state: &AppState,
    tenant_id: i64,
    table: &str,
) -> Result<(TableSchema, Option<String>), AppError> {
    let (schema_json, primary_key) = state
        .db
        .state_table_get(tenant_id, table.to_string())
        .await?
        .ok_or_else(|| table_not_found(table))?;
    let mut schema = parse_schema(&schema_json)?;
    schema.primary_key = primary_key.clone();
    Ok((schema, primary_key))
}

/// requirement 1/AC5: every field the row provides must both be a
/// declared column and match its declared type; nothing is written on the
/// first violation.
fn validate_row(schema: &TableSchema, row: &Map<String, Value>) -> Result<(), AppError> {
    for (field, value) in row {
        let Some(ty) = schema.columns.get(field) else {
            return Err(schema_violation(field, "a declared column"));
        };
        if !ty.matches(value) {
            return Err(schema_violation(field, ty.as_str()));
        }
    }
    Ok(())
}

// ---- host.state.insert/query/delete_rows ---------------------------------

/// requirement 2: `rows` is either one row object or an array of row
/// objects. Each row is validated against the table's declared schema
/// (AC5) before anything is written; if the table has a `primary_key`,
/// inserting a row whose PK value matches an existing row replaces it
/// (the "reads the row, writes the new value" shape the monitor user
/// story needs) rather than accumulating duplicates.
pub async fn state_insert(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let table = arg_str(args, "table")?;
    let (schema, primary_key) = load_table(state, tenant.id, &table).await?;
    let rows_val = args
        .get("rows")
        .ok_or_else(|| AppError::InvalidArgs("missing required argument 'rows'".to_string()))?;
    let rows: Vec<Map<String, Value>> = match rows_val {
        Value::Array(items) => items
            .iter()
            .map(|v| {
                v.as_object()
                    .cloned()
                    .ok_or_else(|| AppError::InvalidArgs("each row must be an object".to_string()))
            })
            .collect::<Result<_, _>>()?,
        Value::Object(obj) => vec![obj.clone()],
        _ => return Err(AppError::InvalidArgs("'rows' must be an object or array of objects".to_string())),
    };
    for row in &rows {
        validate_row(&schema, row)?;
    }

    let plan = plan_of(state, &tenant.plan)?;

    let ops = rows.len() as i64;
    if ops > plan.state_ops_per_call_max {
        return Err(quota_exceeded(
            "state_ops_per_call_max",
            plan.state_ops_per_call_max,
            ops,
        ));
    }

    // requirement 4: `state_rows_max` per table.
    let existing_rows = state.db.state_rows_all(tenant.id, table.clone()).await?;
    let mut row_count = existing_rows.len() as i64;

    let mut total_new_bytes = 0i64;
    let mut inserted_ids = Vec::with_capacity(rows.len());
    for row in rows {
        let row_json = serde_json::to_string(&row)
            .map_err(|e| AppError::Internal(format!("row serialize: {e}")))?;

        // Upsert-on-primary-key: delete any existing row with the same PK
        // value first (net row-count effect is zero for a replace).
        if let Some(pk) = &primary_key
            && let Some(pk_value) = row.get(pk)
        {
            let mut to_delete = Vec::new();
            for (id, existing_json) in &existing_rows {
                if let Ok(existing) = serde_json::from_str::<Value>(existing_json)
                    && existing.get(pk) == Some(pk_value)
                {
                    to_delete.push(*id);
                }
            }
            if !to_delete.is_empty() {
                state
                    .db
                    .state_rows_delete_by_ids(tenant.id, table.clone(), to_delete)
                    .await?;
                row_count -= 1;
            }
        }

        if row_count + 1 > plan.state_rows_max {
            return Err(quota_exceeded("state_rows_max", plan.state_rows_max, row_count));
        }
        total_new_bytes += row_json.len() as i64;
        row_count += 1;
        let id = state.db.state_row_insert(tenant.id, table.clone(), row_json).await?;
        inserted_ids.push(id);
    }

    check_bytes_quota(&state.db, tenant.id, plan, total_new_bytes).await?;

    Ok(json!({"table": table, "inserted": inserted_ids.len(), "ids": inserted_ids}))
}

#[derive(Debug, Clone, Copy)]
enum FilterOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

struct FilterClause {
    field: String,
    op: FilterOp,
    value: Value,
}

/// requirement 2's filter grammar: `field op value`, clauses joined by
/// (case-insensitive) ` and `. `value` is parsed as a JSON scalar when
/// possible (number, `true`/`false`), a single- or double-quoted string
/// when quoted, otherwise a bare string.
fn parse_filter(input: &str) -> Result<Vec<FilterClause>, AppError> {
    let mut clauses = Vec::new();
    for raw_clause in split_and(input) {
        let clause = raw_clause.trim();
        if clause.is_empty() {
            continue;
        }
        let (field, rest) = clause
            .split_once(char::is_whitespace)
            .ok_or_else(|| AppError::InvalidArgs(format!("malformed filter clause: '{clause}'")))?;
        let rest = rest.trim();
        let (op_str, value_str) = rest
            .split_once(char::is_whitespace)
            .ok_or_else(|| AppError::InvalidArgs(format!("malformed filter clause: '{clause}'")))?;
        let op = match op_str {
            "=" => FilterOp::Eq,
            "!=" => FilterOp::Ne,
            "<" => FilterOp::Lt,
            "<=" => FilterOp::Le,
            ">" => FilterOp::Gt,
            ">=" => FilterOp::Ge,
            other => {
                return Err(AppError::InvalidArgs(format!(
                    "unknown filter operator '{other}'; expected one of = != < <= > >="
                )));
            }
        };
        let value_str = value_str.trim();
        let value = parse_filter_value(value_str);
        clauses.push(FilterClause {
            field: field.to_string(),
            op,
            value,
        });
    }
    Ok(clauses)
}

fn split_and(input: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let lower = input.to_ascii_lowercase();
    let mut start = 0usize;
    let mut i = 0usize;
    let bytes = lower.as_bytes();
    while i + 5 <= bytes.len() {
        if &bytes[i..i + 5] == b" and "
            && (i == 0 || bytes[i - 1] != b' ')
        {
            parts.push(&input[start..i]);
            start = i + 5;
            i = start;
        } else {
            i += 1;
        }
    }
    parts.push(&input[start..]);
    parts
}

fn parse_filter_value(raw: &str) -> Value {
    if (raw.starts_with('\'') && raw.ends_with('\'') && raw.len() >= 2)
        || (raw.starts_with('"') && raw.ends_with('"') && raw.len() >= 2)
    {
        return Value::String(raw[1..raw.len() - 1].to_string());
    }
    if let Ok(i) = raw.parse::<i64>() {
        return json!(i);
    }
    if let Ok(f) = raw.parse::<f64>() {
        return json!(f);
    }
    match raw {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        _ => Value::String(raw.to_string()),
    }
}

fn cmp_values(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    if let (Some(x), Some(y)) = (a.as_f64(), b.as_f64()) {
        return x.partial_cmp(&y);
    }
    if let (Some(x), Some(y)) = (a.as_str(), b.as_str()) {
        return Some(x.cmp(y));
    }
    if let (Some(x), Some(y)) = (a.as_bool(), b.as_bool()) {
        return Some(x.cmp(&y));
    }
    None
}

fn row_matches(row: &Value, clauses: &[FilterClause]) -> bool {
    clauses.iter().all(|c| {
        let Some(field_value) = row.get(&c.field) else {
            return false;
        };
        match c.op {
            FilterOp::Eq => field_value == &c.value,
            FilterOp::Ne => field_value != &c.value,
            FilterOp::Lt => cmp_values(field_value, &c.value) == Some(std::cmp::Ordering::Less),
            FilterOp::Le => matches!(
                cmp_values(field_value, &c.value),
                Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
            ),
            FilterOp::Gt => cmp_values(field_value, &c.value) == Some(std::cmp::Ordering::Greater),
            FilterOp::Ge => matches!(
                cmp_values(field_value, &c.value),
                Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
            ),
        }
    })
}

/// requirement 2's `order_by`: `"<field> [asc|desc]"`, defaulting to `asc`
/// when the direction is omitted.
fn parse_order_by(input: &str) -> (String, bool) {
    let input = input.trim();
    if let Some(field) = input.strip_suffix("desc").map(str::trim_end)
        && field != input
    {
        return (field.to_string(), true);
    }
    if let Some(field) = input.strip_suffix("asc").map(str::trim_end)
        && field != input
    {
        return (field.to_string(), false);
    }
    (input.to_string(), false)
}

pub async fn state_query(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let table = arg_str(args, "table")?;
    load_table(state, tenant.id, &table).await?; // 404s if undeclared
    let clauses = match arg_str_opt(args, "where") {
        Some(w) => parse_filter(&w)?,
        None => Vec::new(),
    };
    let rows = state.db.state_rows_all(tenant.id, table.clone()).await?;
    let mut matched: Vec<Value> = rows
        .into_iter()
        .filter_map(|(_, row_json)| serde_json::from_str::<Value>(&row_json).ok())
        .filter(|row| row_matches(row, &clauses))
        .collect();

    if let Some(order_by) = arg_str_opt(args, "order_by") {
        let (field, desc) = parse_order_by(&order_by);
        matched.sort_by(|a, b| {
            let ord = cmp_values(a.get(&field).unwrap_or(&Value::Null), b.get(&field).unwrap_or(&Value::Null))
                .unwrap_or(std::cmp::Ordering::Equal);
            if desc { ord.reverse() } else { ord }
        });
    }

    if let Some(limit) = arg_i64_opt(args, "limit") {
        matched.truncate(limit.max(0) as usize);
    }

    Ok(json!({"table": table, "rows": matched}))
}

pub async fn state_delete_rows(
    state: &AppState,
    tenant: &Tenant,
    args: &Value,
) -> Result<Value, AppError> {
    let table = arg_str(args, "table")?;
    load_table(state, tenant.id, &table).await?;
    let clauses = match arg_str_opt(args, "where") {
        Some(w) => parse_filter(&w)?,
        None => Vec::new(),
    };
    let rows = state.db.state_rows_all(tenant.id, table.clone()).await?;
    let ids: Vec<i64> = rows
        .into_iter()
        .filter_map(|(id, row_json)| {
            let row: Value = serde_json::from_str(&row_json).ok()?;
            row_matches(&row, &clauses).then_some(id)
        })
        .collect();
    let deleted = state.db.state_rows_delete_by_ids(tenant.id, table.clone(), ids).await?;
    Ok(json!({"table": table, "deleted": deleted}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_type_matches_json_shapes() {
        assert!(ColumnType::Text.matches(&json!("hi")));
        assert!(!ColumnType::Text.matches(&json!(1)));
        assert!(ColumnType::Integer.matches(&json!(3)));
        assert!(!ColumnType::Integer.matches(&json!("high")));
        assert!(ColumnType::Real.matches(&json!(3.5)));
        assert!(ColumnType::Real.matches(&json!(3)));
        assert!(ColumnType::Boolean.matches(&json!(true)));
        assert!(!ColumnType::Boolean.matches(&json!("true")));
        assert!(ColumnType::Json.matches(&json!({"a": 1})));
    }

    /// AC5's exact scenario: `last_value: "high"` against a declared
    /// `real` column.
    #[test]
    fn validate_row_rejects_wrong_type_and_names_the_column() {
        let mut columns = std::collections::BTreeMap::new();
        columns.insert("metric".to_string(), ColumnType::Text);
        columns.insert("last_value".to_string(), ColumnType::Real);
        let schema = TableSchema { columns, primary_key: Some("metric".to_string()) };
        let mut row = Map::new();
        row.insert("metric".to_string(), json!("cpu"));
        row.insert("last_value".to_string(), json!("high"));
        let err = validate_row(&schema, &row).unwrap_err();
        assert_eq!(err.code(), "state_schema_violation");
        let data = err.into_error_data().data.expect("data");
        assert_eq!(data["field"], "last_value");
        assert_eq!(data["expected"], "real");
    }

    #[test]
    fn validate_row_rejects_unknown_column() {
        let mut columns = std::collections::BTreeMap::new();
        columns.insert("metric".to_string(), ColumnType::Text);
        let schema = TableSchema { columns, primary_key: None };
        let mut row = Map::new();
        row.insert("nope".to_string(), json!(1));
        assert!(validate_row(&schema, &row).is_err());
    }

    /// AC4's exact scenario: `last_value > 0.5`, `order_by "last_value
    /// desc"`, `limit 2` over three rows.
    #[test]
    fn filter_order_limit_matches_ac4() {
        let clauses = parse_filter("last_value > 0.5").expect("parse");
        let rows = [
            json!({"metric": "a", "last_value": 0.9}),
            json!({"metric": "b", "last_value": 0.2}),
            json!({"metric": "c", "last_value": 0.7}),
        ];
        let mut matched: Vec<&Value> = rows.iter().filter(|r| row_matches(r, &clauses)).collect();
        assert_eq!(matched.len(), 2);
        matched.sort_by(|a, b| {
            cmp_values(&b["last_value"], &a["last_value"]).unwrap_or(std::cmp::Ordering::Equal)
        });
        matched.truncate(2);
        assert_eq!(matched[0]["metric"], "a");
        assert_eq!(matched[1]["metric"], "c");
    }

    #[test]
    fn filter_and_joins_multiple_clauses() {
        let clauses = parse_filter("last_value > 0.5 and acked = false").expect("parse");
        assert_eq!(clauses.len(), 2);
        assert!(row_matches(
            &json!({"last_value": 0.9, "acked": false}),
            &clauses
        ));
        assert!(!row_matches(
            &json!({"last_value": 0.9, "acked": true}),
            &clauses
        ));
    }

    #[test]
    fn order_by_parses_direction() {
        assert_eq!(parse_order_by("last_value desc"), ("last_value".to_string(), true));
        assert_eq!(parse_order_by("last_value asc"), ("last_value".to_string(), false));
        assert_eq!(parse_order_by("last_value"), ("last_value".to_string(), false));
    }

    #[test]
    fn filter_value_parsing() {
        assert_eq!(parse_filter_value("0.5"), json!(0.5));
        assert_eq!(parse_filter_value("3"), json!(3));
        assert_eq!(parse_filter_value("true"), json!(true));
        assert_eq!(parse_filter_value("'eu'"), json!("eu"));
        assert_eq!(parse_filter_value("\"eu\""), json!("eu"));
        assert_eq!(parse_filter_value("eu"), json!("eu"));
    }
}
