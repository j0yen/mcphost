//! Business logic for `host.table.*` (PRD-mcphost-tenant-tables P0
//! requirements 1-6, P1 requirement 7): per-tenant tables backed by a real
//! SQLite database, one file per tenant, so a declared table is a genuine
//! SQL table an agent can run a real read-only `SELECT` against -- not
//! `tenant_state.rs`'s JSON-blob-plus-hand-rolled-filter-grammar KV store
//! (Non-goals: "no replacement of tenant-state; small KV stays KV"; when a
//! tool needs a handful of small values, `host.state.*` is still the right
//! tool -- `host.table.*` is for rows an agent wants to run real SQL over).
//!
//! Technical considerations' open question -- "SQLite-per-tenant vs shared
//! DB with row scoping" -- is resolved here as **SQLite-per-tenant**: each
//! tenant's tables live in their own file,
//! `<data_dir>/tables/<tenant_id>.db`, opened fresh per call (no
//! connection cache kept in [`crate::state::AppState`] -- this is not a hot
//! path, and opening a file this small is well under a millisecond;
//! `busy_timeout` covers the rare case of two concurrent calls from the
//! same tenant). This makes this PRD's isolation requirement (AC3: "the
//! cross-tenant name reads as nonexistent, never as forbidden-but-present")
//! true *structurally* rather than by an access-control check that could
//! have a bug: a `SELECT` naming another tenant's table simply has no such
//! table in the connection it's running against, and SQLite reports
//! exactly the ordinary "no such table" error a typo'd table name gets. It
//! also makes the admin delete cascade (requirement 5) one file removal
//! (see `admin.rs::tenant_delete`), not a set of `DELETE ... WHERE
//! tenant_id` statements that could miss a table.
//!
//! Every declared table's metadata (its column schema and primary key)
//! lives alongside its rows, in the same per-tenant file's own
//! `_mcphost_meta` table -- so a tenant's whole table store is exactly one
//! file to back up, restore, or delete (P1 requirement 8: the existing
//! deploy backup already covers `$MCPHOST_DATA_DIR` as a directory, so a
//! restore drill needs no new steps to also carry `tables/*.db`).
//!
//! `host.table.query`'s read-only enforcement (requirement 2, AC3) is
//! layered, not a single check: (1) `sqlparser` parses the submitted SQL
//! and the call is refused unless it parses to *exactly one* statement
//! whose top-level AST node is `Statement::Query` (a `SELECT`/CTE) -- a
//! structural, parse-level rejection of `UPDATE`/`DROP`/multi-statement
//! input, never a string/keyword match; (2) the per-tenant connection runs
//! with `PRAGMA query_only = ON` for the duration of the call; (3) the
//! compiled statement's own `sqlite3_stmt_readonly()` flag
//! (`Statement::readonly()`) is checked before executing. Any one of the
//! three would already refuse every case this PRD's ACs name; keeping all
//! three is defense in depth against a bug in any single layer, not
//! redundant scope creep.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params, types::ValueRef};
use serde_json::{Map, Value, json};

use crate::db::Tenant;
use crate::errors::AppError;
use crate::plans::Plan;
use crate::state::AppState;

/// requirement 6/AC6: `host.table.query`'s row bound -- a query matching
/// more than this many rows is refused (not silently truncated) naming the
/// bound, so a caller knows to add its own `LIMIT`/`WHERE` rather than
/// silently seeing a partial result it might mistake for complete.
pub const ROW_CAP: i64 = 1_000;

/// requirement 6/AC6: `host.table.query`'s wall-clock bound, enforced via
/// `rusqlite::InterruptHandle` from an independent thread (SQLite's own
/// supported mechanism for aborting a running statement from outside the
/// thread executing it).
pub const QUERY_TIME_CAP: Duration = Duration::from_secs(5);

pub(crate) const META_TABLE: &str = "_mcphost_meta";

/// requirement 1: the small type set `host.table.create`'s `columns`
/// argument may declare. `Timestamp` is stored as `TEXT` (an RFC 3339
/// string the caller provides -- this module does no timezone/format
/// validation of its own, the same permissiveness `Text` already has).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnType {
    Text,
    Integer,
    Real,
    Timestamp,
    Boolean,
    Json,
}

impl ColumnType {
    pub fn as_str(self) -> &'static str {
        match self {
            ColumnType::Text => "text",
            ColumnType::Integer => "integer",
            ColumnType::Real => "real",
            ColumnType::Timestamp => "timestamp",
            ColumnType::Boolean => "boolean",
            ColumnType::Json => "json",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "text" => Some(ColumnType::Text),
            "integer" => Some(ColumnType::Integer),
            "real" => Some(ColumnType::Real),
            "timestamp" => Some(ColumnType::Timestamp),
            "boolean" => Some(ColumnType::Boolean),
            "json" => Some(ColumnType::Json),
            _ => None,
        }
    }

    /// `true` if `value`'s JSON shape matches this declared type -- same
    /// permissive-numeric convention as `tenant_state::ColumnType::matches`
    /// (a bare integer literal is a valid `real`).
    fn matches(self, value: &Value) -> bool {
        match self {
            ColumnType::Text | ColumnType::Timestamp => value.is_string(),
            ColumnType::Integer => value.is_i64() || value.is_u64(),
            ColumnType::Real => value.is_number(),
            ColumnType::Boolean => value.is_boolean(),
            ColumnType::Json => true,
        }
    }

    /// The real SQLite column affinity this type is declared with -- unlike
    /// `tenant_state.rs` (one JSON blob column per row), `host.table.*`
    /// rows land in genuine typed columns so `host.table.query`'s SQL runs
    /// against real SQLite semantics (comparisons, `ORDER BY`, aggregates)
    /// rather than this crate's own hand-rolled filter grammar.
    fn sql_type(self) -> &'static str {
        match self {
            ColumnType::Text | ColumnType::Timestamp | ColumnType::Json => "TEXT",
            ColumnType::Integer | ColumnType::Boolean => "INTEGER",
            ColumnType::Real => "REAL",
        }
    }
}

pub(crate) fn arg_str(args: &Value, name: &str) -> Result<String, AppError> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppError::InvalidArgs(format!("missing required argument '{name}'")))
}

pub(crate) fn arg_str_opt(args: &Value, name: &str) -> Option<String> {
    args.get(name).and_then(Value::as_str).map(str::to_string)
}

fn plan_of<'a>(state: &'a AppState, plan_name: &str) -> Result<&'a Plan, AppError> {
    state.plans.get(plan_name).ok_or_else(|| {
        AppError::Internal(format!(
            "tenant's plan '{plan_name}' is not in the loaded plan catalog"
        ))
    })
}

/// requirement 1: table and column names must be safe to splice, quoted,
/// into DDL/DML -- SQLite has no parameter-binding for identifiers.
/// Deliberately the same shape `AppError::InvalidToolName`'s own
/// `^[a-z][a-z0-9_]{1,40}$` convention uses, widened to allow an
/// underscore-leading or upper-case identifier (schema column names are
/// caller-chosen, not this host's own tool-naming surface).
fn is_valid_ident(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && s.len() <= 64
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Double-quotes `s` as a SQLite identifier, doubling any embedded `"` --
/// belt-and-suspenders alongside [`is_valid_ident`] (which already rejects
/// `"` outright), the same defense-in-depth stance `host.table.query`'s
/// three-layer read-only check takes.
pub(crate) fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

pub(crate) fn table_not_found(table: &str) -> AppError {
    AppError::Structured {
        code: "table_not_found",
        message: format!("no table '{table}' is declared for this tenant"),
        data: json!({"table": table}),
    }
}

fn schema_violation(field: &str, expected: &str) -> AppError {
    AppError::Structured {
        code: "table_schema_violation",
        message: format!("column '{field}' must be of type '{expected}'"),
        data: json!({"field": field, "expected": expected}),
    }
}

fn already_exists(name: &str) -> AppError {
    AppError::Structured {
        code: "table_already_exists",
        message: format!("table '{name}' already exists"),
        data: json!({"table": name}),
    }
}

/// requirement 2/AC3: every structural rejection of a submitted
/// `host.table.query` string -- a parse failure, more than one statement,
/// or a top-level statement that isn't a `SELECT`/CTE.
fn query_rejected(reason: impl Into<String>) -> AppError {
    let reason = reason.into();
    AppError::Structured {
        code: "table_query_rejected",
        message: format!("query rejected: {reason}"),
        data: json!({"reason": reason}),
    }
}

/// requirement 6/AC6: a query that ran past [`ROW_CAP`] or
/// [`QUERY_TIME_CAP`].
fn bound_exceeded(bound: &'static str, limit: i64) -> AppError {
    AppError::Structured {
        code: "table_bound_exceeded",
        message: format!("query exceeded bound '{bound}' (limit {limit})"),
        data: json!({"bound": bound, "limit": limit}),
    }
}

/// requirement 8: `<data_dir>/tables/<tenant_id>.db` -- derived from
/// `Db::data_dir()` (the same directory `mcphost.db` lives in) rather than
/// a second `MCPHOST_DATA_DIR` env read, so tests that point `Db::open` at
/// a scratch dir automatically get a matching scratch `tables/` dir too.
pub(crate) fn tenant_db_path(state: &AppState, tenant_id: i64) -> PathBuf {
    state
        .db
        .data_dir()
        .join("tables")
        .join(format!("{tenant_id}.db"))
}

/// Opens (creating the parent dir and the file if absent) one tenant's
/// table-store connection, in WAL mode with a `_mcphost_meta` bookkeeping
/// table guaranteed to exist -- the per-tenant-file analogue of
/// `Db::open`'s own migration-on-open contract.
/// PRD-mcphost-sqlite-busy-timeout-audit requirement 1: opens through the
/// single factory (role `tenant_table`), which sets `busy_timeout`,
/// `journal_mode=WAL`, `synchronous=NORMAL`, and `foreign_keys=ON`.
fn open_conn(path: &Path, cfg: &crate::db::DbConfig) -> Result<Connection, AppError> {
    let (conn, _audit) = crate::db::open_with_role(path, crate::db::ROLE_TENANT_TABLE, cfg)?;
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {META_TABLE} (
            name TEXT PRIMARY KEY,
            schema_json TEXT NOT NULL,
            primary_key TEXT,
            created_unix INTEGER NOT NULL
        );"
    ))?;
    Ok(conn)
}

/// Runs `f` against a fresh connection to `path` on a blocking thread --
/// the per-tenant-file analogue of `Db::with_conn`, minus the shared-mutex
/// guard (each call gets its own `Connection`; there is no cross-call state
/// to protect beyond what SQLite's own file locking already provides).
pub(crate) async fn with_tenant_conn<F, T>(
    path: PathBuf,
    cfg: crate::db::DbConfig,
    counters: std::sync::Arc<crate::db::DbCounters>,
    f: F,
) -> Result<T, AppError>
where
    F: FnOnce(&Connection) -> Result<T, AppError> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        crate::db::instrument_stmt(&counters, crate::db::ROLE_TENANT_TABLE, move || {
            let conn = open_conn(&path, &cfg)?;
            f(&conn)
        })
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?
}

pub(crate) struct LoadedSchema {
    pub(crate) columns: BTreeMap<String, ColumnType>,
    #[allow(dead_code)] // read for completeness; no caller needs it yet beyond append's upsert-free model
    pub(crate) primary_key: Option<String>,
}

pub(crate) fn load_schema_sync(conn: &Connection, table: &str) -> Result<LoadedSchema, AppError> {
    let row: Option<(String, Option<String>)> = conn
        .query_row(
            &format!("SELECT schema_json, primary_key FROM {META_TABLE} WHERE name = ?1"),
            params![table],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (schema_json, primary_key) = row.ok_or_else(|| table_not_found(table))?;
    let raw: Value = serde_json::from_str(&schema_json)
        .map_err(|e| AppError::Internal(format!("stored schema_json is corrupt: {e}")))?;
    let obj = raw
        .as_object()
        .ok_or_else(|| AppError::Internal("stored schema_json is not an object".to_string()))?;
    let mut columns = BTreeMap::new();
    for (k, v) in obj {
        let ty = v
            .as_str()
            .and_then(ColumnType::parse)
            .ok_or_else(|| {
                AppError::Internal(format!("stored schema_json has a bad type for column '{k}'"))
            })?;
        columns.insert(k.clone(), ty);
    }
    Ok(LoadedSchema { columns, primary_key })
}

pub(crate) fn row_count_sync(conn: &Connection, table: &str) -> Result<i64, AppError> {
    conn.query_row(
        &format!("SELECT COUNT(*) FROM {}", quote_ident(table)),
        [],
        |r| r.get(0),
    )
    .map_err(AppError::from)
}

/// requirement 3 (foreign-key detection): every table name this tenant has
/// declared, in no particular order -- [`crate::tables_model`]'s own
/// candidate list when checking a column against every other table's key.
pub(crate) fn list_table_names_sync(conn: &Connection) -> Result<Vec<String>, AppError> {
    let mut stmt = conn.prepare(&format!("SELECT name FROM {META_TABLE}"))?;
    let names = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(names)
}

// ---- host.table.create / host.table.drop -----------------------------

/// requirement 1: `columns` is `{"column": "text"|"integer"|"real"|
/// "timestamp"|"boolean"|"json", ...}`; `primary_key`, if given, must name
/// one of `columns`'s own entries. requirement 4: refuses past the plan's
/// `table_tables_max`, naming `billing.checkout` (the same upgrade-path
/// shape every other plan quota in this crate already uses).
pub async fn table_create(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let name = arg_str(args, "name")?;
    if !is_valid_ident(&name) {
        return Err(AppError::InvalidArgs(format!(
            "name: must match ^[A-Za-z_][A-Za-z0-9_]{{0,63}}$; got '{name}'"
        )));
    }
    let columns_val = args
        .get("columns")
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| AppError::InvalidArgs("missing required argument 'columns'".to_string()))?;
    if columns_val.is_empty() {
        return Err(AppError::InvalidArgs("'columns' must declare at least one column".to_string()));
    }
    let mut columns: Vec<(String, ColumnType)> = Vec::with_capacity(columns_val.len());
    for (col, ty) in &columns_val {
        if !is_valid_ident(col) {
            return Err(AppError::InvalidArgs(format!(
                "columns: column name '{col}' must match ^[A-Za-z_][A-Za-z0-9_]{{0,63}}$"
            )));
        }
        let ty_str = ty
            .as_str()
            .ok_or_else(|| AppError::InvalidArgs(format!("columns: column '{col}' type must be a string")))?;
        let parsed = ColumnType::parse(ty_str).ok_or_else(|| {
            AppError::InvalidArgs(format!(
                "columns: column '{col}' has unknown type '{ty_str}'; expected one of \
                 text, integer, real, timestamp, boolean, json"
            ))
        })?;
        columns.push((col.clone(), parsed));
    }
    let primary_key = arg_str_opt(args, "primary_key");
    if let Some(pk) = &primary_key
        && !columns.iter().any(|(c, _)| c == pk)
    {
        return Err(AppError::InvalidArgs(format!(
            "primary_key '{pk}' is not one of the declared columns"
        )));
    }

    let plan = plan_of(state, &tenant.plan)?;
    let tables_max = plan.table_tables_max;
    let plan_name = tenant.plan.clone();
    let schema_json = serde_json::to_string(&Value::Object(
        columns.iter().map(|(c, t)| (c.clone(), json!(t.as_str()))).collect(),
    ))
    .map_err(|e| AppError::Internal(format!("schema serialize: {e}")))?;

    let path = tenant_db_path(state, tenant.id);
    let name_for_conn = name.clone();
    with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
        let existing: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM {META_TABLE}"), [], |r| r.get(0))?;
        if existing >= tables_max {
            return Err(crate::billing::quota_exceeded(
                &plan_name,
                "table_tables_max",
                tables_max,
                existing,
                None,
            ));
        }
        let already: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM {META_TABLE} WHERE name = ?1"),
            params![name_for_conn],
            |r| r.get(0),
        )?;
        if already > 0 {
            return Err(already_exists(&name_for_conn));
        }
        let mut col_defs: Vec<String> = columns
            .iter()
            .map(|(c, t)| format!("{} {}", quote_ident(c), t.sql_type()))
            .collect();
        if let Some(pk) = &primary_key {
            col_defs.push(format!("PRIMARY KEY ({})", quote_ident(pk)));
        }
        conn.execute(
            &format!("CREATE TABLE {} ({})", quote_ident(&name_for_conn), col_defs.join(", ")),
            [],
        )?;
        conn.execute(
            &format!(
                "INSERT INTO {META_TABLE} (name, schema_json, primary_key, created_unix) \
                 VALUES (?1, ?2, ?3, ?4)"
            ),
            params![name_for_conn, schema_json, primary_key, crate::state::now_unix()],
        )?;
        Ok(())
    })
    .await?;

    // PRD-mcphost-table-semantic-model requirement 5: `create` marks the
    // model stale too -- a no-op `UPDATE` here (no model exists yet for a
    // table that was just created), kept for symmetry with `append` below.
    // Best-effort: a failure here must never fail the create itself.
    if let Err(e) = state.db.mark_table_model_stale(tenant.id, name.clone()).await {
        tracing::warn!(error = %e, table = %name, "failed to mark table model stale after create");
    }

    Ok(json!({"name": name, "created": true}))
}

pub async fn table_drop(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let name = arg_str(args, "name")?;
    if !is_valid_ident(&name) {
        return Err(table_not_found(&name));
    }
    let path = tenant_db_path(state, tenant.id);
    let name_for_conn = name.clone();
    let dropped = with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
        let existing: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM {META_TABLE} WHERE name = ?1"),
            params![name_for_conn],
            |r| r.get(0),
        )?;
        if existing == 0 {
            return Ok(false);
        }
        conn.execute(&format!("DROP TABLE {}", quote_ident(&name_for_conn)), [])?;
        conn.execute(
            &format!("DELETE FROM {META_TABLE} WHERE name = ?1"),
            params![name_for_conn],
        )?;
        Ok(true)
    })
    .await?;

    // PRD-mcphost-table-semantic-model AC9: a dropped table's model and
    // annotations must not survive it -- `host.table.models` must never
    // list a table that no longer exists. Unconditional (harmless no-op
    // when neither ever existed): `dropped == false` for a name that was
    // never declared, in which case there is nothing to clean up either.
    state.db.delete_table_model_and_annotations(tenant.id, name.clone()).await?;

    Ok(json!({"name": name, "dropped": dropped}))
}

// ---- host.table.append -------------------------------------------------

fn parse_rows_arg(args: &Value) -> Result<Vec<Map<String, Value>>, AppError> {
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
    if rows.is_empty() {
        return Err(AppError::InvalidArgs("'rows' must contain at least one row".to_string()));
    }
    Ok(rows)
}

fn sql_param_for(ty: ColumnType, value: &Value) -> Box<dyn rusqlite::ToSql> {
    match ty {
        ColumnType::Text | ColumnType::Timestamp => {
            Box::new(value.as_str().unwrap_or_default().to_string())
        }
        ColumnType::Integer => Box::new(value.as_i64().unwrap_or_default()),
        ColumnType::Real => Box::new(value.as_f64().unwrap_or_default()),
        ColumnType::Boolean => Box::new(i64::from(value.as_bool().unwrap_or_default())),
        ColumnType::Json => Box::new(serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())),
    }
}

/// requirement 2/AC2: every row is validated against the table's declared
/// schema before anything is written -- a type mismatch or unknown column
/// fails the whole call with `table_schema_violation` and writes nothing
/// (the transaction below is never opened on a validation failure).
fn validate_rows(schema: &LoadedSchema, rows: &[Map<String, Value>]) -> Result<(), AppError> {
    for row in rows {
        for (field, value) in row {
            let Some(ty) = schema.columns.get(field) else {
                return Err(schema_violation(field, "a declared column"));
            };
            if !ty.matches(value) {
                return Err(schema_violation(field, ty.as_str()));
            }
        }
    }
    Ok(())
}

fn insert_rows_sync(
    conn: &Connection,
    table: &str,
    schema: &LoadedSchema,
    rows: &[Map<String, Value>],
) -> Result<Vec<i64>, AppError> {
    conn.execute("BEGIN IMMEDIATE", [])?;
    let outcome: Result<Vec<i64>, AppError> = (|| {
        let mut ids = Vec::with_capacity(rows.len());
        for row in rows {
            let mut cols = Vec::with_capacity(row.len());
            let mut placeholders = Vec::with_capacity(row.len());
            let mut values: Vec<Box<dyn rusqlite::ToSql>> = Vec::with_capacity(row.len());
            for (i, (field, value)) in row.iter().enumerate() {
                cols.push(quote_ident(field));
                placeholders.push(format!("?{}", i + 1));
                let ty = *schema.columns.get(field).expect("validated by validate_rows");
                values.push(sql_param_for(ty, value));
            }
            let sql = format!(
                "INSERT INTO {} ({}) VALUES ({})",
                quote_ident(table),
                cols.join(", "),
                placeholders.join(", "),
            );
            let params_ref: Vec<&dyn rusqlite::ToSql> = values.iter().map(|b| b.as_ref()).collect();
            conn.execute(&sql, params_ref.as_slice())?;
            ids.push(conn.last_insert_rowid());
        }
        Ok(ids)
    })();
    match outcome {
        Ok(ids) => {
            conn.execute("COMMIT", [])?;
            Ok(ids)
        }
        Err(e) => {
            let _ = conn.execute("ROLLBACK", []);
            Err(e)
        }
    }
}

/// requirement 1/3: appends one row (an object) or several (an array of
/// objects) to a declared table. requirement 4: refuses past the plan's
/// `table_rows_max` (this table) or `table_bytes_max` (the tenant's whole
/// table store, approximated by its file size plus the serialized size of
/// the rows about to be written -- the same "measure before writing" stance
/// `tenant_state::check_bytes_quota` takes for the KV store), both checked
/// inside the same connection/transaction as the insert so the check and
/// the write can never race against a second call to this same tenant.
pub async fn table_append(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let table = arg_str(args, "table")?;
    let rows = parse_rows_arg(args)?;

    let plan = plan_of(state, &tenant.plan)?;
    let rows_max = plan.table_rows_max;
    let bytes_max = plan.table_bytes_max;
    let plan_name = tenant.plan.clone();
    let additional_bytes: i64 = rows
        .iter()
        .map(|r| serde_json::to_string(r).map(|s| s.len() as i64).unwrap_or(0))
        .sum();

    let path = tenant_db_path(state, tenant.id);
    let table_for_conn = table.clone();
    let ids = with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
        let schema = load_schema_sync(conn, &table_for_conn)?;
        validate_rows(&schema, &rows)?;

        let existing_rows = row_count_sync(conn, &table_for_conn)?;
        if existing_rows + rows.len() as i64 > rows_max {
            return Err(crate::billing::quota_exceeded(
                &plan_name,
                "table_rows_max",
                rows_max,
                existing_rows,
                None,
            ));
        }
        let file_bytes = conn
            .path()
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|m| m.len() as i64)
            .unwrap_or(0);
        if file_bytes + additional_bytes > bytes_max {
            return Err(crate::billing::quota_exceeded(
                &plan_name,
                "table_bytes_max",
                bytes_max,
                file_bytes,
                None,
            ));
        }

        insert_rows_sync(conn, &table_for_conn, &schema, &rows)
    })
    .await?;

    // PRD-mcphost-table-semantic-model requirement 5: an append marks any
    // existing model for this table stale -- `describe` keeps serving the
    // previous model (with `stale: true`) until the background tick
    // recomputes it; a table never described yet has no model row to mark,
    // which is fine (its first `describe` bootstraps a fresh compute
    // anyway). Best-effort: a failure here must never fail the append.
    if let Err(e) = state.db.mark_table_model_stale(tenant.id, table.clone()).await {
        tracing::warn!(error = %e, table = %table, "failed to mark table model stale after append");
    }

    Ok(json!({"table": table, "appended": ids.len(), "ids": ids}))
}

// ---- host.table.query ---------------------------------------------------

/// requirement 2/AC3: parses `sql` and refuses (structurally, not by string
/// matching) anything but exactly one `SELECT`/CTE statement.
fn validate_query_structure(sql: &str) -> Result<(), AppError> {
    let statements = sqlparser::parser::Parser::parse_sql(&sqlparser::dialect::GenericDialect {}, sql)
        .map_err(|e| query_rejected(format!("sql parse error: {e}")))?;
    match statements.as_slice() {
        [] => Err(query_rejected("empty statement")),
        [sqlparser::ast::Statement::Query(_)] => Ok(()),
        [_single_non_query] => Err(query_rejected(
            "must be a single read-only SELECT statement, not a write or DDL statement",
        )),
        _multiple => Err(query_rejected(
            "multiple statements are not allowed; submit exactly one SELECT",
        )),
    }
}

pub(crate) fn value_ref_to_json(v: ValueRef<'_>) -> Value {
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => json!(i),
        ValueRef::Real(f) => json!(f),
        ValueRef::Text(t) => Value::String(String::from_utf8_lossy(t).into_owned()),
        // No `host.table.create` column is ever declared BLOB; a caller's
        // own SQL expression (e.g. `zeroblob`) could still produce one --
        // degrade to a description rather than erroring the whole query.
        ValueRef::Blob(b) => json!(format!("<blob:{} bytes>", b.len())),
    }
}

/// requirement 6/AC6: enforces [`ROW_CAP`] and [`QUERY_TIME_CAP`], refusing
/// (not truncating) a result that would exceed either. The time bound is
/// enforced by a detached thread calling `InterruptHandle::interrupt()`
/// after the deadline regardless of whether the query already finished --
/// documented-safe on rusqlite's `InterruptHandle` even after the
/// connection it was drawn from is done with (or has dropped) its work, so
/// there is nothing to cancel/join when the query returns first.
fn run_query_sync(conn: &Connection, sql: &str) -> Result<Vec<Value>, AppError> {
    let interrupt = conn.get_interrupt_handle();
    std::thread::spawn(move || {
        std::thread::sleep(QUERY_TIME_CAP);
        interrupt.interrupt();
    });

    conn.pragma_update(None, "query_only", "ON")?;
    let mut stmt = conn.prepare(sql)?;
    if !stmt.readonly() {
        return Err(query_rejected("statement is not read-only"));
    }
    let column_names: Vec<String> = stmt.column_names().into_iter().map(str::to_string).collect();
    let mut rows_out = Vec::new();
    let mut rows = stmt.query([])?;
    loop {
        let row = match rows.next() {
            Ok(Some(r)) => r,
            Ok(None) => break,
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::OperationInterrupted =>
            {
                return Err(bound_exceeded("time_cap_s", QUERY_TIME_CAP.as_secs() as i64));
            }
            Err(e) => return Err(AppError::from(e)),
        };
        if rows_out.len() as i64 >= ROW_CAP {
            return Err(bound_exceeded("row_cap", ROW_CAP));
        }
        let mut obj = Map::new();
        for (i, name) in column_names.iter().enumerate() {
            obj.insert(name.clone(), value_ref_to_json(row.get_ref(i)?));
        }
        rows_out.push(Value::Object(obj));
    }
    Ok(rows_out)
}

pub async fn table_query(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let sql = arg_str(args, "sql")?;
    validate_query_structure(&sql)?;

    let path = tenant_db_path(state, tenant.id);
    let rows = with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| run_query_sync(conn, &sql)).await?;
    Ok(json!({"rows": rows}))
}

// ---- host.table.list / host.table.schema --------------------------------

/// requirement 7: which tables this tenant has declared, with each one's
/// current row count, plus the tenant's whole table-store byte usage (the
/// same file-size measure `table_append`'s quota check uses).
pub async fn table_list(state: &AppState, tenant: &Tenant, _args: &Value) -> Result<Value, AppError> {
    let path = tenant_db_path(state, tenant.id);
    let path_for_size = path.clone();
    let tables = with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
        let mut stmt = conn.prepare(&format!("SELECT name FROM {META_TABLE} ORDER BY name"))?;
        let names: Vec<String> = stmt
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, rusqlite::Error>>()?;
        let mut out = Vec::with_capacity(names.len());
        for name in names {
            let rows = row_count_sync(conn, &name)?;
            out.push(json!({"name": name, "rows": rows}));
        }
        Ok(out)
    })
    .await?;
    let bytes_used = std::fs::metadata(&path_for_size).map(|m| m.len() as i64).unwrap_or(0);
    Ok(json!({"tables": tables, "bytes_used": bytes_used}))
}

/// P1 requirement 7/AC9: one table's columns, types, row count and byte
/// count, without running the caller's own SQL -- `byte count` is the
/// tenant's whole table-store file size (this storage model keeps every
/// declared table in one shared per-tenant file, so a single table's own
/// slice of that isn't cheaply separable without reading every row; see the
/// module doc's SQLite-per-tenant design note).
pub async fn table_schema(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let table = arg_str(args, "table")?;
    let path = tenant_db_path(state, tenant.id);
    let path_for_size = path.clone();
    let table_for_conn = table.clone();
    let (schema, row_count) = with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
        let schema = load_schema_sync(conn, &table_for_conn)?;
        let row_count = row_count_sync(conn, &table_for_conn)?;
        Ok((schema, row_count))
    })
    .await?;
    let columns: Map<String, Value> = schema
        .columns
        .iter()
        .map(|(k, v)| (k.clone(), json!(v.as_str())))
        .collect();
    let bytes_used = std::fs::metadata(&path_for_size).map(|m| m.len() as i64).unwrap_or(0);
    Ok(json!({
        "table": table,
        "columns": columns,
        "rows": row_count,
        "bytes_used": bytes_used,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bare `AppState` over a scratch data dir -- the same literal
    /// `tests/runs_ac11_admin_runs_reap.rs`'s own `bare_state()` builds
    /// (deliberately duplicated here rather than shared: that helper lives
    /// in an integration test crate this lib crate's unit tests can't
    /// import from).
    async fn bare_state(dir: &Path) -> AppState {
        let db = crate::db::Db::open(dir).expect("open db");
        db.migrate().await.expect("migrate");
        AppState {
            db,
            kinds: crate::kinds::KindRegistry::with_builtin(),
            secrets: crate::secrets::SecretBox::from_passphrase("test-secret-key"),
            admin_key: Some("test-admin-key".to_string()),
            public_url: "http://127.0.0.1:0".to_string(),
            call_timeout: crate::state::CALL_TIMEOUT,
            registry: None,
            http_client: reqwest::Client::new(),
            sandbox_mechanism: None,
            wasm_runtime_version: None,
            tool_run_limiter: crate::state::ToolRunLimiter::new(),
            signup_rate_limit_per_hour: crate::state::SIGNUP_RATE_LIMIT_PER_HOUR,
            plans: crate::plans::PlanCatalog::default_catalog(),
            billing_config: crate::billing::BillingConfig::default(),
            billing_client: std::sync::Arc::new(crate::billing::FakeBillingClient::new(
                crate::state::now_unix(),
            )),
            checkout_sessions: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            accepted_usage_cache: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            runs: crate::runs::RunsRegistry::new(),
            scheduler: crate::triggers::SchedulerStatus::new(),
            event_counters: crate::hooks::EventCounters::new(),
            event_rate_limiter: crate::hooks::EventRateLimiter::new(),
            deprecations: std::sync::Arc::new(Vec::new()),
            disk_guard: crate::retention::DiskGuard::from_env(),
            compat_token: None,
            signup_pause: crate::state::SignupPause::from_env(dir),
            claim_token_ttl_secs: crate::state::CLAIM_TOKEN_TTL_SECS_DEFAULT,
            claim_rate_limit_per_hour: crate::state::CLAIM_RATE_LIMIT_PER_HOUR_DEFAULT,
            email_config: crate::email::EmailConfig::default(),
            email_client: std::sync::Arc::new(crate::email::FakeEmailClient::new()),
            bans: crate::bans::BanCache::new(),
            ban_denials_threshold: crate::bans::BAN_DENIALS_THRESHOLD_DEFAULT,
            ban_claim_rate_threshold: crate::bans::BAN_CLAIM_RATE_THRESHOLD_DEFAULT,
            oauth: crate::oauth::JwksCache::new(),
            oauth_allowed_algs: crate::oauth::parse_allowed_algs(None),
            oauth_jwks_ttl_secs: crate::oauth::DEFAULT_JWKS_TTL_SECS,
            alerts: crate::alerts::AlertRegistry::new(),
            contention_tracker: crate::alerts::ContentionTracker::new(),
            alert_config: crate::alerts::AlertConfig::default(),
            alert_quota_trips: crate::alerts::QuotaTripTracker::new(),
            status_probe_override: crate::statusfeed::ProbeOverrides::new(),
        }
    }

    async fn test_tenant(state: &AppState, namespace: &str) -> Tenant {
        state
            .db
            .create_tenant(
                format!("{namespace} display"),
                namespace.to_string(),
                format!("key-hash-{namespace}"),
                None,
            )
            .await
            .expect("create_tenant")
    }

    fn scratch_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mcphost-tables-test-{label}-{}-{}",
            std::process::id(),
            crate::state::now_unix_ms()
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn valid_ident_accepts_and_rejects() {
        assert!(is_valid_ident("metrics"));
        assert!(is_valid_ident("_metrics_1"));
        assert!(!is_valid_ident("1metrics"));
        assert!(!is_valid_ident("metrics;drop"));
        assert!(!is_valid_ident(""));
        assert!(!is_valid_ident("has space"));
    }

    #[test]
    fn column_type_matches_json_shapes() {
        assert!(ColumnType::Text.matches(&json!("hi")));
        assert!(!ColumnType::Text.matches(&json!(1)));
        assert!(ColumnType::Timestamp.matches(&json!("2026-09-13T00:00:00Z")));
        assert!(ColumnType::Integer.matches(&json!(3)));
        assert!(!ColumnType::Integer.matches(&json!("high")));
        assert!(ColumnType::Real.matches(&json!(3.5)));
        assert!(ColumnType::Real.matches(&json!(3)));
        assert!(ColumnType::Boolean.matches(&json!(true)));
        assert!(!ColumnType::Boolean.matches(&json!("true")));
        assert!(ColumnType::Json.matches(&json!({"a": 1})));
    }

    /// requirement 2/AC3: a parse error, an UPDATE, a DROP, and a
    /// multi-statement string are all rejected structurally.
    #[test]
    fn query_structure_rejects_non_select_and_multi_statement() {
        assert!(validate_query_structure("select * from t").is_ok());
        assert!(validate_query_structure("SELECT * FROM t WHERE x > 1").is_ok());
        assert!(validate_query_structure("with c as (select 1) select * from c").is_ok());
        assert_eq!(
            validate_query_structure("update t set x = 1").unwrap_err().code(),
            "table_query_rejected"
        );
        assert_eq!(
            validate_query_structure("drop table t").unwrap_err().code(),
            "table_query_rejected"
        );
        assert_eq!(
            validate_query_structure("select 1; select 2").unwrap_err().code(),
            "table_query_rejected"
        );
        assert_eq!(
            validate_query_structure("select * from t where (").unwrap_err().code(),
            "table_query_rejected"
        );
    }

    /// AC1's exact scenario: create a table, append 3 valid rows, and query
    /// them with a SELECT and a WHERE -- results return under `rows` with
    /// correct values, ordered as the query asks.
    #[tokio::test]
    async fn create_append_query_round_trip() {
        let dir = scratch_dir("roundtrip");
        let state = bare_state(&dir).await;
        let t = test_tenant(&state, "t1").await;

        table_create(
            &state,
            &t,
            &json!({"name": "metrics", "columns": {"metric": "text", "value": "real"}}),
        )
        .await
        .expect("create");

        table_append(
            &state,
            &t,
            &json!({"table": "metrics", "rows": [
                {"metric": "cpu", "value": 0.9},
                {"metric": "mem", "value": 0.2},
                {"metric": "disk", "value": 0.7},
            ]}),
        )
        .await
        .expect("append");

        let result = table_query(
            &state,
            &t,
            &json!({"sql": "SELECT metric, value FROM metrics WHERE value > 0.5 ORDER BY value DESC"}),
        )
        .await
        .expect("query");
        let rows = result["rows"].as_array().expect("rows array");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["metric"], "cpu");
        assert_eq!(rows[1]["metric"], "disk");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// AC2's exact scenario: a wrong type, a missing/unknown column --
    /// refused, naming the column, no partial rows land.
    #[tokio::test]
    async fn append_rejects_schema_violation_and_writes_nothing() {
        let dir = scratch_dir("schema-violation");
        let state = bare_state(&dir).await;
        let t = test_tenant(&state, "t1").await;
        table_create(
            &state,
            &t,
            &json!({"name": "metrics", "columns": {"metric": "text", "value": "real"}}),
        )
        .await
        .expect("create");

        let err = table_append(
            &state,
            &t,
            &json!({"table": "metrics", "rows": [
                {"metric": "cpu", "value": 0.9},
                {"metric": "mem", "value": "high"},
            ]}),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), "table_schema_violation");

        let result = table_query(&state, &t, &json!({"sql": "SELECT * FROM metrics"}))
            .await
            .expect("query");
        assert!(
            result["rows"].as_array().expect("rows").is_empty(),
            "no partial rows must land on a schema violation"
        );

        let err2 = table_append(
            &state,
            &t,
            &json!({"table": "metrics", "rows": [{"nope": 1}]}),
        )
        .await
        .unwrap_err();
        assert_eq!(err2.code(), "table_schema_violation");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// AC3's cross-tenant scenario: a query naming a table that exists for
    /// a different tenant reads as an ordinary "no such table" -- the
    /// per-tenant-file isolation makes this structural, not access-control.
    #[tokio::test]
    async fn cross_tenant_table_name_reads_as_nonexistent() {
        let dir = scratch_dir("isolation");
        let state = bare_state(&dir).await;
        let tenant_a = test_tenant(&state, "tenant-a").await;
        let tenant_b = test_tenant(&state, "tenant-b").await;
        table_create(
            &state,
            &tenant_a,
            &json!({"name": "secrets_table", "columns": {"v": "text"}}),
        )
        .await
        .expect("create for tenant a");

        let err = table_query(
            &state,
            &tenant_b,
            &json!({"sql": "SELECT * FROM secrets_table"}),
        )
        .await
        .unwrap_err();
        // Structurally a plain SQLite "no such table" -- surfaced as
        // `storage` (this module's generic rusqlite-error passthrough),
        // never a distinct "forbidden" code that would leak the table's
        // existence to the wrong tenant.
        assert_eq!(err.code(), "storage");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// requirement 4/AC4: a plan with `table_tables_max` reached refuses a
    /// further create naming `billing.checkout`.
    #[tokio::test]
    async fn table_quota_refuses_past_tables_max() {
        let dir = scratch_dir("quota-tables");
        let mut state = bare_state(&dir).await;
        for p in &mut state.plans.plans {
            if p.name == "free" {
                p.table_tables_max = 1;
            }
        }
        let t = test_tenant(&state, "quota-tables").await;

        table_create(&state, &t, &json!({"name": "a", "columns": {"x": "text"}}))
            .await
            .expect("first table ok");
        let err = table_create(&state, &t, &json!({"name": "b", "columns": {"x": "text"}}))
            .await
            .unwrap_err();
        assert_eq!(err.code(), "quota_exceeded");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// requirement 6/AC6: a query matching more rows than `ROW_CAP` is
    /// refused naming the bound, not silently truncated.
    #[tokio::test]
    async fn query_row_cap_refuses_oversized_result() {
        let dir = scratch_dir("row-cap");
        let state = bare_state(&dir).await;
        let t = test_tenant(&state, "t1").await;
        table_create(&state, &t, &json!({"name": "big", "columns": {"n": "integer"}}))
            .await
            .expect("create");
        let rows: Vec<Value> = (0..(ROW_CAP + 5)).map(|n| json!({"n": n})).collect();
        table_append(&state, &t, &json!({"table": "big", "rows": rows}))
            .await
            .expect("append past cap (row_cap only bounds query results, not appends)");

        let err = table_query(&state, &t, &json!({"sql": "SELECT * FROM big"}))
            .await
            .unwrap_err();
        assert_eq!(err.code(), "table_bound_exceeded");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn drop_removes_table_and_meta_row() {
        let dir = scratch_dir("drop");
        let state = bare_state(&dir).await;
        let t = test_tenant(&state, "t1").await;
        table_create(&state, &t, &json!({"name": "temp", "columns": {"x": "text"}}))
            .await
            .expect("create");
        let result = table_drop(&state, &t, &json!({"name": "temp"})).await.expect("drop");
        assert_eq!(result["dropped"], true);

        let err = table_append(&state, &t, &json!({"table": "temp", "rows": [{"x": "y"}]}))
            .await
            .unwrap_err();
        assert_eq!(err.code(), "table_not_found");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn schema_reports_columns_and_row_count_without_querying() {
        let dir = scratch_dir("schema");
        let state = bare_state(&dir).await;
        let t = test_tenant(&state, "t1").await;
        table_create(
            &state,
            &t,
            &json!({"name": "metrics", "columns": {"metric": "text", "value": "real"}}),
        )
        .await
        .expect("create");
        table_append(
            &state,
            &t,
            &json!({"table": "metrics", "rows": [{"metric": "cpu", "value": 0.5}]}),
        )
        .await
        .expect("append");

        let result = table_schema(&state, &t, &json!({"table": "metrics"})).await.expect("schema");
        assert_eq!(result["rows"], 1);
        assert_eq!(result["columns"]["metric"], "text");
        assert_eq!(result["columns"]["value"], "real");

        std::fs::remove_dir_all(&dir).ok();
    }
}
