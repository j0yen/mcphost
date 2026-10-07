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
use crate::enduser::EndUser;
use crate::errors::AppError;
use crate::plans::Plan;
use crate::rowpolicy;
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

/// PRD-mcphost-table-context-and-sql-passthrough requirement 2/3: every
/// `host.table.query` call, whether it returns rows or is refused, gets one
/// row here -- reserved under the same leading-underscore convention
/// [`META_TABLE`] already uses, so `host.table.create`/`host.table.drop`
/// can never declare or drop it (neither checks this name against
/// [`META_TABLE`]'s own rows, so `create` hits SQLite's own "table already
/// exists" and `drop` finds no matching row to remove -- structural, like
/// [`META_TABLE`]'s own reservation).
pub(crate) const QUERY_LOG_TABLE: &str = "_mcphost_query_log";

/// PRD-mcphost-drift-review requirement 2: `host.docs.search`'s own call
/// log, in the same per-tenant-file pattern as [`QUERY_LOG_TABLE`] --
/// `drift::rerun`'s dependency lookup for a changed document scans this
/// table (within the last 30 days) for a search whose top hits named it.
pub(crate) const DOCS_SEARCH_LOG_TABLE: &str = "_docs_search_log";

/// requirement 3: the query log keeps at most this many rows per tenant;
/// the insert that would make one more evicts the oldest in the same
/// transaction.
pub const QUERY_LOG_CAP: i64 = 1_000;

/// requirement 2 (P2 requirement 10/AC13): `sql` longer than this is stored
/// truncated (with `truncated: true`) rather than refused for length.
const QUERY_LOG_SQL_MAX_BYTES: usize = 4_096;

/// requirement 4: `host.table.query_log`'s default and max `limit`.
pub const QUERY_LOG_LIMIT_DEFAULT: i64 = 50;
pub const QUERY_LOG_LIMIT_CAP: i64 = 200;

/// PRD-mcphost-query-diagnosis requirement 6: `host.table.query_stats`'s
/// `window_s` default (24h) and cap (7d).
pub(crate) const QUERY_STATS_WINDOW_DEFAULT_S: i64 = 86_400;
pub(crate) const QUERY_STATS_WINDOW_MAX_S: i64 = 604_800;

/// PRD-mcphost-result-handles requirement 1/6: a `hdl_` name is reserved
/// for query-result handles -- `host.table.create`/`host.table.drop`
/// refuse it (AC6), and it's how [`table_query`] tells a handle name
/// apart from a declared table when deciding whether a "no such table"
/// ought to read as `handle_not_found` instead.
pub const HANDLE_PREFIX: &str = "hdl_";

/// requirement 1: a handle's own id half (after [`HANDLE_PREFIX`]) is this
/// many lowercase base32 characters.
const HANDLE_ID_LEN: usize = 12;

/// Lowercase RFC 4648 base32 alphabet -- requirement 1's "12 lowercase
/// base32 characters", the same bit-packing approach `state::new_ulid`
/// already uses for Crockford base32, just a different (lowercase)
/// alphabet and no leading timestamp half (a handle's id carries no
/// ordering promise the way a ULID's does).
const HANDLE_ID_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz234567";

/// requirement 2: a handle's summary carries a sample of at most this many
/// rows.
pub const HANDLE_SAMPLE_CAP: i64 = 20;

/// requirement 4: `ttl_s`'s default when omitted.
pub const HANDLE_TTL_DEFAULT_S: i64 = 3_600;

/// requirement 4: `ttl_s`'s ceiling -- a caller asking for longer is
/// refused, not silently clamped.
pub const HANDLE_TTL_MAX_S: i64 = 86_400;

/// requirement 4: the tables tick drops an expired handle within this long
/// of its `expires_unix` -- see `spawn_tick`.
const HANDLE_TICK_INTERVAL_SECS: u64 = 30;

pub(crate) const HANDLES_META_TABLE: &str = "_mcphost_handles";

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
pub(crate) fn open_conn(path: &Path, cfg: &crate::db::DbConfig) -> Result<Connection, AppError> {
    // PRD-mcphost-result-handles requirement 4/AC4: "`bytes_used` fell
    // accordingly" after a handle (or a dropped declared table) is gone
    // needs the freed pages actually returned to the file's own free
    // space on commit, not just added to SQLite's internal freelist
    // inside an unchanged file size -- `auto_vacuum = FULL`. `open_with_role`
    // itself already performs a write (the journal_mode=WAL pragma) that
    // disqualifies changing auto_vacuum afterwards without a `VACUUM`
    // (same constraint the main db's own `migrate_0026_retention` already
    // documents), so this checks *before* opening whether this tenant's
    // file exists yet and, only the first time, rebuilds it under the new
    // mode immediately -- a `VACUUM` over an empty database is trivial,
    // and every call after the first (this file already exists) skips it.
    let is_new = !path.exists();
    let (conn, _audit) = crate::db::open_with_role(path, crate::db::ROLE_TENANT_TABLE, cfg)?;
    if is_new {
        conn.pragma_update(None, "auto_vacuum", "FULL")?;
        conn.execute_batch("VACUUM;")?;
    }
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {META_TABLE} (
            name TEXT PRIMARY KEY,
            schema_json TEXT NOT NULL,
            primary_key TEXT,
            created_unix INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS {QUERY_LOG_TABLE} (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            created_unix INTEGER NOT NULL,
            sql TEXT NOT NULL,
            truncated INTEGER NOT NULL DEFAULT 0,
            row_count INTEGER,
            duration_ms INTEGER NOT NULL,
            error_code TEXT,
            error_message TEXT
        );
        CREATE TABLE IF NOT EXISTS {DOCS_SEARCH_LOG_TABLE} (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            created_unix INTEGER NOT NULL,
            query TEXT NOT NULL,
            mode TEXT NOT NULL,
            top_ids_json TEXT NOT NULL
        );"
    ))?;
    // PRD-mcphost-drift-review requirement 4: `sample_hash` is additive on
    // an existing tenant's `_mcphost_query_log` (created by the dependency
    // PRD before this column existed) -- same `pragma_table_info` guard
    // `Db::migrate_0002_tenant_last_tool_change`'s own doc comment explains
    // SQLite needs for an `ALTER TABLE ADD COLUMN` to stay idempotent.
    let has_sample_hash: bool = conn
        .prepare(&format!("SELECT 1 FROM pragma_table_info('{QUERY_LOG_TABLE}') WHERE name = 'sample_hash'"))?
        .exists([])?;
    if !has_sample_hash {
        conn.execute(&format!("ALTER TABLE {QUERY_LOG_TABLE} ADD COLUMN sample_hash TEXT"), [])?;
    }
    // PRD-mcphost-query-diagnosis requirement 3/7: additive on an existing
    // tenant's `_mcphost_query_log` (created before this feature) -- same
    // idempotent-`ALTER TABLE` guard as `sample_hash` above.
    let has_diagnosis: bool = conn
        .prepare(&format!("SELECT 1 FROM pragma_table_info('{QUERY_LOG_TABLE}') WHERE name = 'diagnosis'"))?
        .exists([])?;
    if !has_diagnosis {
        conn.execute(&format!("ALTER TABLE {QUERY_LOG_TABLE} ADD COLUMN diagnosis TEXT"), [])?;
        conn.execute(&format!("ALTER TABLE {QUERY_LOG_TABLE} ADD COLUMN hint TEXT"), [])?;
        conn.execute(&format!("ALTER TABLE {QUERY_LOG_TABLE} ADD COLUMN result_bytes INTEGER"), [])?;
        conn.execute(&format!("ALTER TABLE {QUERY_LOG_TABLE} ADD COLUMN est_tokens INTEGER"), [])?;
    }
    // PRD-mcphost-result-handles requirement 1: one row per live
    // `host.table.query {handle: true}` handle, alongside `META_TABLE` in
    // the same per-tenant file -- a handle's own table (`handle` doubles
    // as its name, since a handle name *is* the `hdl_<id>` table name) is
    // never described in `META_TABLE` (it's not a declared `host.table.*`
    // table), so this is a second bookkeeping table rather than an
    // overload of the first.
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {HANDLES_META_TABLE} (
            handle TEXT PRIMARY KEY,
            created_unix INTEGER NOT NULL,
            expires_unix INTEGER NOT NULL,
            sql TEXT NOT NULL,
            bytes INTEGER NOT NULL,
            row_count INTEGER NOT NULL,
            last_used_unix INTEGER NOT NULL
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
    // PRD-mcphost-dry-run-side-effects requirement 2: inside a
    // `dryrun::with_dry_run` scope, every table op for this call routes
    // through that call's own dedicated, savepoint-wrapped connection
    // (`dryrun::DryRunCtx::table_conn_sync`) instead of a fresh one -- so a
    // `mcphost.table.append` followed by a `mcphost.table.query` in the same
    // call sees the uncommitted append, and neither is ever committed to the
    // tenant's own `tables/<id>.db` file.
    let dry_run = crate::dryrun::current();
    tokio::task::spawn_blocking(move || {
        crate::db::instrument_stmt(&counters, crate::db::ROLE_TENANT_TABLE, move || match &dry_run
        {
            Some(dry_run) => {
                let conn = dry_run.table_conn_sync(&path, &cfg)?;
                let guard = conn
                    .lock()
                    .map_err(|_| AppError::Storage("db lock poisoned".into()))?;
                f(&guard)
            }
            None => {
                let conn = open_conn(&path, &cfg)?;
                f(&conn)
            }
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
    // PRD-mcphost-result-handles requirement 6/AC6: `hdl_` names a query
    // result handle, never a declared table.
    if name.starts_with(HANDLE_PREFIX) {
        return Err(AppError::InvalidArgs(format!(
            "name: the '{HANDLE_PREFIX}' prefix is reserved for query result handles"
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
    // PRD-mcphost-upgrade-moment requirement 1: `quota_exceeded`'s `next`
    // block needs the plan catalog and billing mode, but this whole
    // closure crosses into `with_tenant_conn`'s `spawn_blocking` task
    // (`Send + 'static`), so a cloned catalog travels with it rather than
    // a borrowed `&AppState`.
    let catalog = state.plans.clone();
    let billing_mode = state.billing_config.billing_mode();
    let schema_json = serde_json::to_string(&Value::Object(
        columns.iter().map(|(c, t)| (c.clone(), json!(t.as_str()))).collect(),
    ))
    .map_err(|e| AppError::Internal(format!("schema serialize: {e}")))?;

    let path = tenant_db_path(state, tenant.id);
    let name_for_conn = name.clone();
    with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
        let existing: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM {META_TABLE}"), [], |r| r.get(0))?;
        if existing >= tables_max {
            return Err(crate::billing::quota_exceeded_for(
                &catalog,
                billing_mode,
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
    // requirement 6/AC6: `hdl_` names a handle -- `host.table.handle_drop`
    // drops one, this tool never does.
    if name.starts_with(HANDLE_PREFIX) {
        return Err(AppError::InvalidArgs(format!(
            "name: the '{HANDLE_PREFIX}' prefix is reserved for query result handles"
        )));
    }
    let confirm = args.get("confirm").and_then(Value::as_bool).unwrap_or(false);

    // PRD-mcphost-lineage-blast-radius requirement 7 (AC2/AC4): gate
    // placement is before the transaction below, so a refusal costs one
    // graph load and never touches the tenant's table file. AC2: a table
    // with no breaking consumers drops exactly as before -- `breaking` is
    // empty, this whole block is skipped, no refusal is written.
    let breaking = crate::lineage::breaking_drop_consumers(state, tenant.id, &name).await?;
    if !breaking.is_empty() && !confirm {
        let consumers: Vec<Value> = breaking
            .iter()
            .take(10)
            .map(|n| json!({"id": n.id, "kind": n.kind.as_str(), "label": n.label, "uses": n.uses}))
            .collect();
        let _ = state
            .db
            .record_tenant_audit(
                tenant.id,
                "table".to_string(),
                "table_drop_refused".to_string(),
                Some(format!("table '{name}' drop refused: {} breaking consumer(s)", breaking.len())),
            )
            .await;
        return Err(AppError::Structured {
            code: "lineage_blocked",
            message: format!(
                "dropping '{name}' would break {} consumer(s); call again with confirm: true to proceed",
                breaking.len()
            ),
            data: json!({"lineage_blocked": consumers}),
        });
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

    // PRD-mcphost-lineage-blast-radius requirement 7 (AC4): a confirmed
    // drop over breaking consumers gets its own audit line (the refusal
    // path above already wrote one for the refused attempt); cascade
    // removes the table's own lineage node/edges and orphans dependent
    // chart/handle nodes.
    if dropped {
        if !breaking.is_empty() {
            let _ = state
                .db
                .record_tenant_audit(
                    tenant.id,
                    "table".to_string(),
                    "table_drop_confirmed".to_string(),
                    Some(format!(
                        "table '{name}' dropped with confirm: true over {} breaking consumer(s)",
                        breaking.len()
                    )),
                )
                .await;
        }
        if let Err(e) = crate::lineage::cascade_table_drop(state, tenant.id, &name).await {
            tracing::warn!(error = %e, table = %name, "failed to cascade lineage cleanup after table drop");
        }
    }

    // PRD-mcphost-table-concept-graph requirement 2: a dropped table's
    // nodes/edges must not survive it in the graph either -- best-effort,
    // same stance as `table_append`'s own mark above.
    if let Err(e) = state.db.mark_table_graph_stale(tenant.id).await {
        tracing::warn!(error = %e, table = %name, "failed to mark table graph stale after drop");
    }


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
    // PRD-mcphost-dry-run-side-effects: under `dryrun::DryRunCtx`, `conn`
    // already has `SAVEPOINT test_run` open (requirement 2's "savepoint
    // depth 1" -- this is the one nested level the DB layer already uses),
    // so a plain `BEGIN IMMEDIATE` here would fail with "cannot start a
    // transaction within a transaction". A nested `SAVEPOINT` is always
    // legal whether or not an outer transaction is active, but it defers
    // lock acquisition (unlike `BEGIN IMMEDIATE`'s eager one) -- so the
    // ordinary (non-nested) path keeps `BEGIN IMMEDIATE`'s eager locking
    // unchanged, and only the nested case (`!conn.is_autocommit()`) takes
    // the savepoint form.
    let nested = !conn.is_autocommit();
    if nested {
        conn.execute("SAVEPOINT insert_rows", [])?;
    } else {
        conn.execute("BEGIN IMMEDIATE", [])?;
    }
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
            if nested {
                conn.execute("RELEASE insert_rows", [])?;
            } else {
                conn.execute("COMMIT", [])?;
            }
            Ok(ids)
        }
        Err(e) => {
            if nested {
                let _ = conn.execute("ROLLBACK TO insert_rows", []);
                let _ = conn.execute("RELEASE insert_rows", []);
            } else {
                let _ = conn.execute("ROLLBACK", []);
            }
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
    // PRD-mcphost-upgrade-moment requirement 1: same cloned-catalog
    // rationale as `table_create`'s own `tables_max` check above -- this
    // closure also crosses into `with_tenant_conn`'s `spawn_blocking` task.
    let catalog = state.plans.clone();
    let billing_mode = state.billing_config.billing_mode();
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
            return Err(crate::billing::quota_exceeded_for(
                &catalog,
                billing_mode,
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
            return Err(crate::billing::quota_exceeded_for(
                &catalog,
                billing_mode,
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
    // PRD-mcphost-table-concept-graph requirement 2/AC6: an append also
    // marks this tenant's graph stale -- it was built from the model this
    // append just staled, so it's stale too until the same tick rebuilds
    // both. Best-effort, same stance as the model-stale mark above.
    if let Err(e) = state.db.mark_table_graph_stale(tenant.id).await {
        tracing::warn!(error = %e, table = %table, "failed to mark table graph stale after append");
    }

    Ok(json!({"table": table, "appended": ids.len(), "ids": ids}))
}

// ---- host.table.query ---------------------------------------------------

/// requirement 2/AC3: parses `sql` and refuses (structurally, not by string
/// matching) anything but exactly one `SELECT`/CTE statement -- returning
/// the parsed [`sqlparser::ast::Query`] so PRD-mcphost-row-policy's AST
/// rewrite (below) can reuse this same parse rather than parsing twice.
pub(crate) fn validate_query_structure(sql: &str) -> Result<Box<sqlparser::ast::Query>, AppError> {
    let mut statements = sqlparser::parser::Parser::parse_sql(&sqlparser::dialect::GenericDialect {}, sql)
        .map_err(|e| query_rejected(format!("sql parse error: {e}")))?;
    match statements.len() {
        0 => Err(query_rejected("empty statement")),
        1 => match statements.remove(0) {
            sqlparser::ast::Statement::Query(q) => Ok(q),
            _ => Err(query_rejected(
                "must be a single read-only SELECT statement, not a write or DDL statement",
            )),
        },
        _ => Err(query_rejected(
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
pub(crate) fn run_query_sync(conn: &Connection, sql: &str) -> Result<Vec<Value>, AppError> {
    run_query_sync_with_bindings(conn, sql, &[])
}

/// PRD-mcphost-row-policy requirement 4: same enforcement as
/// [`run_query_sync`], but binds `bindings` (`":name"` -> value) on the
/// prepared statement -- the row-policy AST rewrite's predicate values
/// never appear as literals in `sql` itself, only as bound parameters.
fn run_query_sync_with_bindings(
    conn: &Connection,
    sql: &str,
    bindings: &[(String, Value)],
) -> Result<Vec<Value>, AppError> {
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
    let sql_values: Vec<(String, rusqlite::types::Value)> =
        bindings.iter().map(|(k, v)| (k.clone(), json_to_sql_value(v))).collect();
    let param_refs: Vec<(&str, &dyn rusqlite::ToSql)> =
        sql_values.iter().map(|(k, v)| (k.as_str(), v as &dyn rusqlite::ToSql)).collect();
    let mut rows = if param_refs.is_empty() { stmt.query([])? } else { stmt.query(&param_refs[..])? };
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

/// PRD-mcphost-drift-review requirement 4: a hash of the first 20 rows, in
/// the order returned -- `drift::rerun`'s own `before`/`after` comparison
/// for a re-run query, and what [`table_query`] stores alongside `row_count`
/// at log time so there's something to compare a re-run's result against
/// later (the original rows themselves are never kept, only this hash).
pub(crate) fn sample_hash_of(rows: &[Value]) -> String {
    let sample = &rows[..rows.len().min(20)];
    let serialized = serde_json::to_string(sample).unwrap_or_default();
    crate::billing::sha256_hex(serialized.as_bytes())
}

/// requirement 2 (P2 requirement 10/AC13): truncates `sql` to at most
/// [`QUERY_LOG_SQL_MAX_BYTES`] bytes (on a UTF-8 char boundary) for storage
/// in the log -- the submitted query itself is never refused for length,
/// only the stored copy is shortened.
fn truncate_sql_for_log(sql: &str) -> (String, bool) {
    if sql.len() <= QUERY_LOG_SQL_MAX_BYTES {
        return (sql.to_string(), false);
    }
    let mut end = QUERY_LOG_SQL_MAX_BYTES;
    while end > 0 && !sql.is_char_boundary(end) {
        end -= 1;
    }
    (sql[..end].to_string(), true)
}

/// [`log_query`]'s own outcome fields, bundled so the function stays under
/// this repo's `too-many-arguments-threshold = 5` (clippy.toml) without a
/// suppression.
///
/// PRD-mcphost-query-diagnosis P0 requirement 3 / P1 requirement 7:
/// `diagnosis`/`hint` (identifier-coverage diagnosis, computed by
/// [`diagnose_sync`]) and `result_bytes`/`est_tokens` (the result's
/// footprint) join the original fields here.
struct QueryLogOutcome<'a> {
    duration_ms: i64,
    row_count: Option<i64>,
    sample_hash: Option<&'a str>,
    error: Option<&'a AppError>,
    diagnosis: Option<&'a str>,
    hint: Option<&'a str>,
    result_bytes: Option<i64>,
    est_tokens: Option<i64>,
}

/// requirement 2/3: appends one row to this connection's query log,
/// evicting the oldest row past [`QUERY_LOG_CAP`] in the same transaction.
/// requirement 2's "logging failure never fails the query" -- any error
/// writing the log row is warned and swallowed here, never propagated to
/// the caller of [`table_query`].
fn log_query(conn: &Connection, sql: &str, log_outcome: QueryLogOutcome<'_>) {
    let QueryLogOutcome { duration_ms, row_count, sample_hash, error, diagnosis, hint, result_bytes, est_tokens } =
        log_outcome;
    let (stored_sql, truncated) = truncate_sql_for_log(sql);
    // requirement 6/AC6: `bound_exceeded` gives every bound refusal the
    // same `table_bound_exceeded` top-level code (that's what the caller's
    // JSON-RPC error carries), but `query_stats`'s `refused` breakdown
    // needs to tell a row-cap refusal from a time-cap one -- so the logged
    // (and therefore stats-keying) code is the specific `data.bound` name
    // when present, falling back to the top-level code otherwise.
    let error_code = error.map(|e| match e {
        AppError::Structured { code: "table_bound_exceeded", data, .. } => {
            data.get("bound").and_then(Value::as_str).unwrap_or("table_bound_exceeded")
        }
        _ => e.code(),
    });
    let error_message = error.map(ToString::to_string);
    let outcome: rusqlite::Result<()> = (|| {
        conn.execute("BEGIN IMMEDIATE", [])?;
        let write: rusqlite::Result<()> = (|| {
            conn.execute(
                &format!(
                    "INSERT INTO {QUERY_LOG_TABLE} \
                     (created_unix, sql, truncated, row_count, duration_ms, error_code, error_message, \
                      sample_hash, diagnosis, hint, result_bytes, est_tokens) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"
                ),
                params![
                    crate::state::now_unix(),
                    stored_sql,
                    truncated as i64,
                    row_count,
                    duration_ms,
                    error_code,
                    error_message,
                    sample_hash,
                    diagnosis,
                    hint,
                    result_bytes,
                    est_tokens,
                ],
            )?;
            conn.execute(
                &format!(
                    "DELETE FROM {QUERY_LOG_TABLE} WHERE id NOT IN \
                     (SELECT id FROM {QUERY_LOG_TABLE} ORDER BY id DESC LIMIT ?1)"
                ),
                params![QUERY_LOG_CAP],
            )?;
            Ok(())
        })();
        match write {
            Ok(()) => conn.execute("COMMIT", []).map(|_| ()),
            Err(e) => {
                let _ = conn.execute("ROLLBACK", []);
                Err(e)
            }
        }
    })();
    if let Err(e) = outcome {
        tracing::warn!(error = %e, "failed to write host.table.query log row");
    }
}

/// PRD-mcphost-query-diagnosis requirement 1: what [`table_query_select`] learns
/// from a pure (no IO) AST parse, before opening the per-tenant connection
/// -- `column_annotation_candidates` is the one piece that genuinely needs
/// the main database (requirement 2/AC7: a column's `description`
/// annotation, read async via [`crate::db::Db::list_table_model_annotations`]),
/// so it is fetched up front and carried into the sync/blocking closure
/// alongside the AST's own findings.
struct QueryDiagContext {
    from_table: Option<String>,
    where_equalities: Vec<(String, String)>,
    column_annotation_candidates: Vec<(String, String)>,
}

/// PRD-mcphost-row-policy requirement 2: converts a resolved rule/literal
/// value into the `rusqlite` value it's bound as.
fn json_to_sql_value(v: &Value) -> rusqlite::types::Value {
    match v {
        Value::Null => rusqlite::types::Value::Null,
        Value::Bool(b) => rusqlite::types::Value::Integer(if *b { 1 } else { 0 }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                rusqlite::types::Value::Integer(i)
            } else if let Some(f) = n.as_f64() {
                rusqlite::types::Value::Real(f)
            } else {
                rusqlite::types::Value::Null
            }
        }
        Value::String(s) => rusqlite::types::Value::Text(s.clone()),
        other => rusqlite::types::Value::Text(other.to_string()),
    }
}

/// PRD-mcphost-row-policy requirement 2: substitutes `compiled`'s `{pN}`
/// template placeholders with fresh `:<tag>N` bound-parameter names, for a
/// one-off `SELECT COUNT(*)` against `table` rather than the AST rewrite
/// (used by [`withheld_count_sync`] below, which only ever queries one
/// table directly, never a rewritten user query).
fn predicate_count_sql(
    table: &str,
    compiled: &rowpolicy::CompiledPolicy,
    tag: &str,
) -> (String, Vec<(String, Value)>) {
    let mut predicate = compiled.rls_predicate.clone();
    let mut bindings = Vec::new();
    for (i, v) in compiled.rls_params.iter().enumerate() {
        let placeholder = format!("{{p{i}}}");
        let name = format!(":{tag}{i}");
        predicate = predicate.replace(&placeholder, &name);
        bindings.push((name, v.clone()));
    }
    let quoted = table.replace('"', "\"\"");
    (format!("SELECT COUNT(*) FROM \"{quoted}\" WHERE {predicate}"), bindings)
}

/// requirement 6: "`withheld_count` is rows... the unfiltered read would
/// have returned, computed only when under 10,000 candidates, else null" --
/// computed here as the policied table's own unfiltered row count minus
/// the count its compiled predicate admits, not the outer query's own
/// (possibly aggregated) result shape.
fn withheld_count_sync(
    conn: &Connection,
    table: &str,
    compiled: &rowpolicy::CompiledPolicy,
) -> Result<Option<i64>, AppError> {
    let quoted = table.replace('"', "\"\"");
    let total: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM \"{quoted}\""), [], |r| r.get(0))?;
    if total >= 10_000 {
        return Ok(None);
    }
    if compiled.rls_predicate == "1 = 0" {
        return Ok(Some(total));
    }
    let (sql, bindings) = predicate_count_sql(table, compiled, "wc");
    let sql_values: Vec<(String, rusqlite::types::Value)> =
        bindings.iter().map(|(k, v)| (k.clone(), json_to_sql_value(v))).collect();
    let param_refs: Vec<(&str, &dyn rusqlite::ToSql)> =
        sql_values.iter().map(|(k, v)| (k.as_str(), v as &dyn rusqlite::ToSql)).collect();
    let admitted: i64 = conn.query_row(&sql, &param_refs[..], |r| r.get(0))?;
    Ok(Some(total - admitted))
}


/// PRD-mcphost-query-diagnosis P0 requirements 1/2/4 (AC1/AC2/AC3/AC7):
/// computes this query's diagnosis, purely from data already in reach of
/// the per-tenant connection plus `ctx` -- never a second query execution,
/// never a model call. Returns `(diagnosis_json, hint)`, both `None` when
/// there is nothing to diagnose (a successful non-empty result: AC4; a
/// structural refusal that isn't a SQLite bind error, e.g. the UPDATE
/// `table_query_rejected` case PRD-mcphost-table-context-and-sql-passthrough's
/// own AC5 covers; a zero-row result with no text-column equality filter
/// to check).
fn diagnose_sync(conn: &Connection, ctx: &QueryDiagContext, result: &Result<Vec<Value>, AppError>) -> (Option<Value>, Option<String>) {
    // Wrapped in an inner closure purely so every early-exit below can use
    // `?` on `Option` ("no diagnosis for this case") without fighting this
    // function's own `(Option<Value>, Option<String>)` return type.
    let diag: Option<crate::query_diag::IdentifierDiagnosis> = (|| {
        match result {
            Err(e) => {
                let message = e.to_string();
                let (kind, ident) = crate::query_diag::parse_bind_error(&message)?;
                match kind {
                    "table" => {
                        let all_tables = list_table_names_sync(conn).unwrap_or_default();
                        let candidates: Vec<(String, String)> =
                            all_tables.iter().map(|t| (t.clone(), t.clone())).collect();
                        Some(crate::query_diag::score_identifier(&ident, "table", &candidates))
                    }
                    "column" => {
                        let table = ctx.from_table.as_ref()?;
                        let schema = load_schema_sync(conn, table).ok()?;
                        let mut candidates: Vec<(String, String)> =
                            schema.columns.keys().map(|c| (c.clone(), c.clone())).collect();
                        candidates.extend(ctx.column_annotation_candidates.iter().cloned());
                        Some(crate::query_diag::score_identifier(&ident, "column", &candidates))
                    }
                    _ => None,
                }
            }
            Ok(rows) if rows.is_empty() => {
                let table = ctx.from_table.as_ref()?;
                let schema = load_schema_sync(conn, table).ok()?;
                let (col, literal) = ctx
                    .where_equalities
                    .iter()
                    .find(|(c, _)| matches!(schema.columns.get(c), Some(ColumnType::Text)))?;
                let mut stmt = conn
                    .prepare(&format!(
                        "SELECT DISTINCT {} FROM {} LIMIT {}",
                        quote_ident(col),
                        quote_ident(table),
                        QUERY_DIAG_DISTINCT_VALUES_BOUND
                    ))
                    .ok()?;
                let distinct: Vec<String> = stmt
                    .query_map([], |r| r.get::<_, Option<String>>(0))
                    .ok()?
                    .filter_map(|r| r.ok().flatten())
                    .collect();
                Some(crate::query_diag::score_value(literal, &distinct))
            }
            Ok(_) => None,
        }
    })();

    let Some(diag) = diag else {
        return (None, None);
    };
    let hint = crate::query_diag::build_hint(&diag, ctx.from_table.as_deref());
    let diagnosis_json = json!({"identifiers": [diag.to_json()], "band": diag.band.as_str()});
    (Some(diagnosis_json), hint)
}

/// PRD-mcphost-query-diagnosis requirement 4: distinct-value sample bound
/// for value diagnosis, re-exported here (rather than imported per call
/// site) purely so the SQL string literal above reads as a named constant.
const QUERY_DIAG_DISTINCT_VALUES_BOUND: i64 = crate::query_diag::DISTINCT_VALUES_BOUND;


/// requirement 4: `host.table.query_log(limit?, before_id?)` -- this
/// tenant's own query log, newest first, `limit` defaulting to
/// [`QUERY_LOG_LIMIT_DEFAULT`] and capped at [`QUERY_LOG_LIMIT_CAP`], paging
/// by `before_id`. Reads only the calling tenant's own per-tenant file --
/// there is no argument that names another tenant (the same structural
/// isolation [`table_query_select`] itself relies on).
///
/// The full `_mcphost_query_log` column list, shared by every reader
/// (`host.table.query_log`, `host.table.query_diagnose`,
/// `host.table.query_stats`) so they can never drift from each other.
const QUERY_LOG_COLS: &str = "id, created_unix, sql, truncated, row_count, duration_ms, error_code, \
     error_message, diagnosis, hint, result_bytes, est_tokens";

/// Parses one `QUERY_LOG_COLS`-ordered row into its wire JSON shape --
/// `diagnosis` is stored as a JSON string and parsed back into a JSON
/// value here (never left as a double-encoded string) so a caller reading
/// either `query_log` or `query_diagnose` gets the same structured shape.
fn query_log_row_to_json(r: &rusqlite::Row) -> rusqlite::Result<Value> {
    let diagnosis_str: Option<String> = r.get(8)?;
    let diagnosis = diagnosis_str.and_then(|s| serde_json::from_str::<Value>(&s).ok());
    Ok(json!({
        "id": r.get::<_, i64>(0)?,
        "created_unix": r.get::<_, i64>(1)?,
        "sql": r.get::<_, String>(2)?,
        "truncated": r.get::<_, i64>(3)? != 0,
        "row_count": r.get::<_, Option<i64>>(4)?,
        "duration_ms": r.get::<_, i64>(5)?,
        "error_code": r.get::<_, Option<String>>(6)?,
        "error_message": r.get::<_, Option<String>>(7)?,
        "diagnosis": diagnosis,
        "hint": r.get::<_, Option<String>>(9)?,
        "result_bytes": r.get::<_, Option<i64>>(10)?,
        "est_tokens": r.get::<_, Option<i64>>(11)?,
    }))
}

pub async fn table_query_log(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let limit = args
        .get("limit")
        .and_then(Value::as_i64)
        .unwrap_or(QUERY_LOG_LIMIT_DEFAULT)
        .clamp(1, QUERY_LOG_LIMIT_CAP);
    let before_id = args.get("before_id").and_then(Value::as_i64);

    let path = tenant_db_path(state, tenant.id);
    let rows = with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
        let to_row = query_log_row_to_json;
        let rows = match before_id {
            Some(before) => {
                let mut stmt = conn.prepare(&format!(
                    "SELECT {QUERY_LOG_COLS} FROM {QUERY_LOG_TABLE} WHERE id < ?1 ORDER BY id DESC LIMIT ?2"
                ))?;
                stmt.query_map(params![before, limit], to_row)?
                    .collect::<rusqlite::Result<Vec<Value>>>()?
            }
            None => {
                let mut stmt = conn.prepare(&format!(
                    "SELECT {QUERY_LOG_COLS} FROM {QUERY_LOG_TABLE} ORDER BY id DESC LIMIT ?1"
                ))?;
                stmt.query_map(params![limit], to_row)?
                    .collect::<rusqlite::Result<Vec<Value>>>()?
            }
        };
        Ok(rows)
    })
    .await?;
    Ok(json!({"rows": rows}))
}

/// PRD-mcphost-query-diagnosis P0 requirement 5/AC5: `host.table.query_diagnose(log_id)`
/// -- this tenant's own log rows only, scoped by the per-tenant-file
/// isolation [`table_query_select`]'s own cross-tenant test already relies on: a
/// `log_id` from a different tenant's file simply has no matching row
/// here, structurally `not_found` the same way a cross-tenant table name
/// reads as nonexistent rather than forbidden.
pub async fn table_query_diagnose(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let log_id = args
        .get("log_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| AppError::InvalidArgs("missing required argument 'log_id'".to_string()))?;

    let path = tenant_db_path(state, tenant.id);
    let row = with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
        conn.query_row(
            &format!("SELECT {QUERY_LOG_COLS} FROM {QUERY_LOG_TABLE} WHERE id = ?1"),
            params![log_id],
            query_log_row_to_json,
        )
        .optional()
        .map_err(AppError::from)
    })
    .await?;
    let Some(row) = row else {
        return Err(AppError::Structured {
            code: "not_found",
            message: format!("no query log row '{log_id}' for this tenant"),
            data: json!({"log_id": log_id}),
        });
    };

    if !row["diagnosis"].is_null() || !row["hint"].is_null() {
        return Ok(json!({"log_id": log_id, "diagnosis": row["diagnosis"], "hint": row["hint"]}));
    }

    // requirement 5: "recomputing them against the current schema if the
    // row predates this feature" -- reruns [`diagnose_sync`]'s own logic
    // from the row's own stored sql/error/row_count, never by re-running
    // the query itself.
    let sql = row["sql"].as_str().unwrap_or_default().to_string();
    let error_message = row["error_message"].as_str().map(str::to_string);
    let row_count = row["row_count"].as_i64();

    let extracted = crate::query_diag::extract_from_ast(&sql);
    let column_annotation_candidates = if let Some(table) = &extracted.from_table {
        let annotations = state
            .db
            .list_table_model_annotations(tenant.id, table.clone())
            .await
            .unwrap_or_default();
        annotations
            .iter()
            .filter(|a| !a.column_name.is_empty() && a.key == "description")
            .flat_map(|a| crate::query_diag::annotation_word_candidates(&a.column_name, &a.value))
            .collect()
    } else {
        Vec::new()
    };
    let ctx = QueryDiagContext {
        from_table: extracted.from_table,
        where_equalities: extracted.where_equalities,
        column_annotation_candidates,
    };

    // A synthetic re-creation of the original call's `Result` shape --
    // [`diagnose_sync`] only inspects an `Err`'s message text (via
    // `query_diag::parse_bind_error`) and an `Ok`'s emptiness, never any
    // other field, so this round-trips exactly what it needs from the
    // stored row without re-running the query.
    let synthetic_result: Result<Vec<Value>, AppError> = if let Some(msg) = error_message {
        Err(AppError::Structured { code: "storage", message: msg, data: Value::Null })
    } else if row_count.unwrap_or(0) == 0 {
        Ok(Vec::new())
    } else {
        Ok(vec![Value::Null])
    };

    let path2 = tenant_db_path(state, tenant.id);
    let (diagnosis, hint) = with_tenant_conn(path2, state.db.cfg(), state.db.counters_handle(), move |conn| {
        Ok(diagnose_sync(conn, &ctx, &synthetic_result))
    })
    .await?;

    Ok(json!({"log_id": log_id, "diagnosis": diagnosis, "hint": hint}))
}

/// PRD-mcphost-query-diagnosis P0 requirement 6/AC6: `host.table.query_stats(window_s?)`
/// -- counts, outcome breakdown, latency percentiles, result footprint,
/// and the top 5 most common hints over the calling tenant's own query
/// log rows created within the last `window_s` seconds (default 24h,
/// capped at 7d). Computed from whatever the capped log currently
/// retains (`QUERY_LOG_MAX_ROWS`): a tenant querying fast enough to evict
/// rows out of the window sees a smaller `queries` count than it issued,
/// the same honest-about-its-own-retention stance `query_log` itself
/// takes.
pub async fn table_query_stats(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let window_s = args
        .get("window_s")
        .and_then(Value::as_i64)
        .unwrap_or(QUERY_STATS_WINDOW_DEFAULT_S)
        .clamp(1, QUERY_STATS_WINDOW_MAX_S);
    let since = crate::state::now_unix() - window_s;

    let path = tenant_db_path(state, tenant.id);
    with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
        let mut stmt = conn.prepare(&format!(
            "SELECT row_count, duration_ms, error_code, hint, est_tokens FROM {QUERY_LOG_TABLE} \
             WHERE created_unix >= ?1"
        ))?;
        // A named type alias, not an inline 5-tuple in the `let` binding
        // below, so clippy's `type_complexity` lint has nothing to flag --
        // `src/tables.rs` is on `checkcompat_race_ac07`'s fixed file list,
        // which forbids a new clippy allow-attribute (clean by
        // construction, not by suppression).
        type StatsRow = (Option<i64>, i64, Option<String>, Option<String>, Option<i64>);
        let rows: Vec<StatsRow> = stmt
            .query_map(params![since], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let queries = rows.len() as i64;
        let mut ok = 0i64;
        let mut empty = 0i64;
        let mut refused: BTreeMap<String, i64> = BTreeMap::new();
        let mut durations: Vec<i64> = Vec::with_capacity(rows.len());
        let mut result_rows_total: i64 = 0;
        let mut est_result_tokens_total: i64 = 0;
        let mut hint_counts: BTreeMap<String, i64> = BTreeMap::new();

        for (row_count, duration_ms, error_code, hint, est_tokens) in &rows {
            durations.push(*duration_ms);
            match (error_code, row_count) {
                (Some(code), _) => *refused.entry(code.clone()).or_insert(0) += 1,
                (None, Some(0)) => empty += 1,
                (None, _) => ok += 1,
            }
            if let Some(rc) = row_count {
                result_rows_total += rc;
            }
            if let Some(et) = est_tokens {
                est_result_tokens_total += et;
            }
            if let Some(h) = hint {
                *hint_counts.entry(h.clone()).or_insert(0) += 1;
            }
        }
        durations.sort_unstable();
        let percentile = |p: f64| -> i64 {
            if durations.is_empty() {
                return 0;
            }
            let idx = ((durations.len() as f64) * p).ceil() as usize;
            let idx = idx.saturating_sub(1).min(durations.len() - 1);
            durations[idx]
        };

        let mut top_hints: Vec<(String, i64)> = hint_counts.into_iter().collect();
        top_hints.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        top_hints.truncate(5);

        Ok(json!({
            "queries": queries,
            "ok": ok,
            "empty": empty,
            "refused": Value::Object(refused.into_iter().map(|(k, v)| (k, json!(v))).collect()),
            "p50_ms": percentile(0.50),
            "p95_ms": percentile(0.95),
            "result_rows_total": result_rows_total,
            "est_result_tokens_total": est_result_tokens_total,
            "top_hints": top_hints.into_iter().map(|(hint, count)| json!({"hint": hint, "count": count})).collect::<Vec<_>>(),
        }))
    })
    .await
}

/// requirement 3/AC3/AC11: a query referencing a `hdl_` name this
/// connection's file has no live row for -- never created, expired and
/// already dropped by the tick, or (structurally, same isolation
/// [`tables.rs`'s module doc already gives declared tables) belonging to a
/// different tenant's own file.
fn handle_not_found(handle: &str) -> AppError {
    AppError::Structured {
        code: "handle_not_found",
        message: format!("no handle '{handle}' exists for this tenant"),
        data: json!({"handle": handle}),
    }
}

/// requirement 4/AC5: a single materialisation whose own bytes alone
/// exceed the tenant's plan's [`crate::plans::Plan::table_handle_bytes_max`]
/// -- refused outright (the transaction this runs inside is rolled back by
/// the caller), naming the cap so the caller knows why nothing was
/// created.
fn handle_quota_exceeded(plan: &str, cap_bytes: i64) -> AppError {
    AppError::Structured {
        code: "handle_quota_exceeded",
        message: format!("plan '{plan}' handle byte quota exceeded (cap {cap_bytes} bytes)"),
        data: json!({"plan": plan, "cap_bytes": cap_bytes}),
    }
}

/// requirement 1: a fresh 12-character lowercase-base32 handle id --
/// 60 bits of randomness via `rand` (already a dependency; same
/// bit-packing approach `state::new_ulid` uses for its own base32 half),
/// packed 5 bits per character.
fn generate_handle_id() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut bytes);
    let acc = u64::from_be_bytes(bytes);
    let mut out = String::with_capacity(HANDLE_ID_LEN);
    for i in (0..HANDLE_ID_LEN).rev() {
        let shift = i * 5;
        let idx = ((acc >> shift) & 0x1f) as usize;
        out.push(HANDLE_ID_ALPHABET[idx] as char);
    }
    out
}

/// Every table name `sql` references in a `FROM`/`JOIN` position (CTEs
/// followed into their own body, derived subqueries recursed into) --
/// technical considerations: "the parser gate already yields table
/// names". Column/`WHERE`-subquery references are not walked: every ACs'
/// own scenario names a handle directly in `FROM`/a top-level CTE, and
/// this is used only to widen [`handle_not_found`]'s check and bump
/// `last_used_unix` (requirement/AC5's eviction order), neither of which
/// needs to be exhaustive over every SQL position a table name could
/// theoretically appear in.
fn collect_referenced_tables(query: &sqlparser::ast::Query, out: &mut std::collections::HashSet<String>) {
    if let Some(with) = &query.with {
        for cte in &with.cte_tables {
            collect_referenced_tables(&cte.query, out);
        }
    }
    collect_set_expr(&query.body, out);
}

fn collect_set_expr(body: &sqlparser::ast::SetExpr, out: &mut std::collections::HashSet<String>) {
    use sqlparser::ast::SetExpr;
    match body {
        SetExpr::Select(select) => {
            for twj in &select.from {
                collect_table_factor(&twj.relation, out);
                for join in &twj.joins {
                    collect_table_factor(&join.relation, out);
                }
            }
        }
        SetExpr::Query(q) => collect_referenced_tables(q, out),
        SetExpr::SetOperation { left, right, .. } => {
            collect_set_expr(left, out);
            collect_set_expr(right, out);
        }
        _ => {}
    }
}

fn collect_table_factor(tf: &sqlparser::ast::TableFactor, out: &mut std::collections::HashSet<String>) {
    use sqlparser::ast::TableFactor;
    match tf {
        TableFactor::Table { name, .. } => {
            out.insert(name.to_string().trim_matches('"').to_lowercase());
        }
        TableFactor::Derived { subquery, .. } => collect_referenced_tables(subquery, out),
        TableFactor::NestedJoin { table_with_joins, .. } => {
            collect_table_factor(&table_with_joins.relation, out);
            for j in &table_with_joins.joins {
                collect_table_factor(&j.relation, out);
            }
        }
        _ => {}
    }
}

/// [`validate_query_structure`] already proved `sql` parses to exactly one
/// `SELECT`/CTE statement; this re-parses (cheap -- these are short
/// strings) to walk it for [`collect_referenced_tables`].
fn referenced_table_names(sql: &str) -> Result<std::collections::HashSet<String>, AppError> {
    let mut statements = sqlparser::parser::Parser::parse_sql(&sqlparser::dialect::GenericDialect {}, sql)
        .map_err(|e| query_rejected(format!("sql parse error: {e}")))?;
    let mut out = std::collections::HashSet::new();
    if !statements.is_empty()
        && let sqlparser::ast::Statement::Query(q) = statements.remove(0)
    {
        collect_referenced_tables(&q, &mut out);
    }
    Ok(out)
}

/// Runs `sql` (already known read-only) and collects every row as
/// [`value_ref_to_json`] objects, with no [`ROW_CAP`]/[`QUERY_TIME_CAP`]
/// enforcement of its own -- the small, bounded reads
/// [`dataset_summary_sync`] and callers inside an already-capped
/// materialisation use (a handle's own sample/stats queries, never a
/// caller-facing arbitrary SQL string).
fn select_all_sync(conn: &Connection, sql: &str) -> Result<Vec<Value>, AppError> {
    let mut stmt = conn.prepare(sql)?;
    let column_names: Vec<String> = stmt.column_names().into_iter().map(str::to_string).collect();
    let mut out = Vec::new();
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let mut obj = Map::new();
        for (i, name) in column_names.iter().enumerate() {
            obj.insert(name.clone(), value_ref_to_json(row.get_ref(i)?));
        }
        out.push(Value::Object(obj));
    }
    Ok(out)
}

/// requirement 4: a handle's byte footprint -- `SUM(pgsize)` over SQLite's
/// `dbstat` virtual table (compiled in; see `libsqlite3-sys`'s build.rs)
/// filtered to this one table's own pages, the real per-table page
/// accounting `table_append`'s whole-file-size approximation can't give
/// (declared tables share one file-wide byte quota, so that approximation
/// was always good enough there; a handle's own quota is per-handle).
fn handle_bytes_sync(conn: &Connection, table: &str) -> Result<i64, AppError> {
    conn.query_row(
        "SELECT COALESCE(SUM(pgsize), 0) FROM dbstat WHERE name = ?1",
        params![table],
        |r| r.get(0),
    )
    .map_err(AppError::from)
}

fn column_dtype_sync(conn: &Connection, table: &str, col: &str) -> Result<&'static str, AppError> {
    let ty: Option<String> = conn
        .query_row(
            &format!(
                "SELECT typeof({}) FROM {} WHERE {} IS NOT NULL LIMIT 1",
                quote_ident(col),
                quote_ident(table),
                quote_ident(col),
            ),
            [],
            |r| r.get(0),
        )
        .optional()?;
    Ok(match ty.as_deref() {
        Some("integer") => "integer",
        Some("real") => "real",
        Some("text") => "text",
        Some("blob") => "blob",
        _ => "null",
    })
}

/// requirement 2: one column's `{min, max, sum, mean, distinct, top_k}` --
/// one `SELECT` for the first five (technical considerations), `sum`/
/// `mean` only for a numeric `dtype` (left `null` otherwise), plus one
/// `GROUP BY ... LIMIT 5` for `top_k`. AC12: over an empty handle, every
/// aggregate is SQL `NULL` and `distinct` is `0`.
fn column_stats_sync(conn: &Connection, table: &str, col: &str, dtype: &str) -> Result<Value, AppError> {
    let q = quote_ident(col);
    let t = quote_ident(table);
    let numeric = dtype == "integer" || dtype == "real";
    let sum_expr = if numeric { format!("SUM({q})") } else { "NULL".to_string() };
    let avg_expr = if numeric { format!("AVG({q})") } else { "NULL".to_string() };
    let (min_v, max_v, sum_v, mean_v, distinct_v) = conn.query_row(
        &format!("SELECT MIN({q}), MAX({q}), {sum_expr}, {avg_expr}, COUNT(DISTINCT {q}) FROM {t}"),
        [],
        |r| {
            Ok((
                value_ref_to_json(r.get_ref(0)?),
                value_ref_to_json(r.get_ref(1)?),
                value_ref_to_json(r.get_ref(2)?),
                value_ref_to_json(r.get_ref(3)?),
                r.get::<_, i64>(4)?,
            ))
        },
    )?;

    let top_k: Vec<Value> = select_all_sync(
        conn,
        &format!("SELECT {q} AS value, COUNT(*) AS count FROM {t} GROUP BY {q} ORDER BY count DESC LIMIT 5"),
    )?
    .into_iter()
    .map(|row| json!({"value": row["value"], "count": row["count"]}))
    .collect();

    Ok(json!({
        "min": min_v,
        "max": max_v,
        "sum": sum_v,
        "mean": mean_v,
        "distinct": distinct_v,
        "top_k": top_k,
    }))
}

/// requirement 2: `dataset-summary.v1` -- built entirely from real SQL
/// over the freshly-materialised handle, never from the sample (Goals 2:
/// "The handle's summary is bounded and honest: a sample, and stats over
/// all rows").
fn dataset_summary_sync(
    conn: &Connection,
    handle: &str,
    sql: &str,
    bytes: i64,
    expires_unix: i64,
) -> Result<Value, AppError> {
    let row_count = row_count_sync(conn, handle)?;
    let column_names: Vec<String> = {
        let stmt = conn.prepare(&format!("SELECT * FROM {} LIMIT 0", quote_ident(handle)))?;
        stmt.column_names().into_iter().map(str::to_string).collect()
    };

    let mut columns_meta = Vec::with_capacity(column_names.len());
    let mut stats = Map::new();
    for col in &column_names {
        let dtype = column_dtype_sync(conn, handle, col)?;
        columns_meta.push(json!({"name": col, "dtype": dtype, "nullable": true}));
        stats.insert(col.clone(), column_stats_sync(conn, handle, col, dtype)?);
    }

    let sample = select_all_sync(
        conn,
        &format!("SELECT * FROM {} LIMIT {HANDLE_SAMPLE_CAP}", quote_ident(handle)),
    )?;

    Ok(json!({
        "handle": handle,
        "table": handle,
        "row_count": row_count,
        "columns": columns_meta,
        "sample": sample,
        "sample_cap": HANDLE_SAMPLE_CAP,
        "stats": stats,
        "bytes": bytes,
        "expires_unix": expires_unix,
        "derived_from": sql,
    }))
}

/// requirement 4/AC5: evicts the least-recently-queried live handles
/// (`last_used_unix` ascending; `rowid` ascending breaks a tie between two
/// handles touched in the same second) until `new_bytes` fits alongside
/// whatever remains under `cap` -- called only once the new handle's own
/// bytes have already been checked to fit under `cap` alone, so this
/// always terminates (in the worst case, every existing handle is
/// evicted).
fn evict_lru_handles_until_fits_sync(conn: &Connection, new_bytes: i64, cap: i64) -> Result<(), AppError> {
    loop {
        let existing_total: i64 = conn.query_row(
            &format!("SELECT COALESCE(SUM(bytes), 0) FROM {HANDLES_META_TABLE}"),
            [],
            |r| r.get(0),
        )?;
        if existing_total + new_bytes <= cap {
            return Ok(());
        }
        let victim: Option<String> = conn
            .query_row(
                &format!(
                    "SELECT handle FROM {HANDLES_META_TABLE} ORDER BY last_used_unix ASC, rowid ASC LIMIT 1"
                ),
                [],
                |r| r.get(0),
            )
            .optional()?;
        let Some(victim) = victim else { return Ok(()) };
        drop_handle_row_sync(conn, &victim)?;
    }
}

fn drop_handle_row_sync(conn: &Connection, handle: &str) -> Result<(), AppError> {
    conn.execute(&format!("DROP TABLE IF EXISTS {}", quote_ident(handle)), [])?;
    conn.execute(
        &format!("DELETE FROM {HANDLES_META_TABLE} WHERE handle = ?1"),
        params![handle],
    )?;
    Ok(())
}

/// [`materialize_handle_sync`]'s own per-call parameters beyond `conn`/
/// `sql` -- bundled into one struct (rather than four more positional
/// arguments) to stay under this crate's `clippy::too_many_arguments`
/// threshold without a suppression, same "a named-field struct beats a
/// positional call site" call `db::RejectedRun` already makes.
struct MaterializeParams<'a> {
    ttl_s: i64,
    now: i64,
    handle_bytes_max: i64,
    plan_name: &'a str,
}

/// requirement 1/2/4: `host.table.query {sql, handle: true, ttl_s?}` --
/// runs `sql` (still gated by [`validate_query_structure`], AC7: the same
/// structural read-only check every other query goes through) as
/// `CREATE TABLE hdl_<id> AS <sql>` on a connection that never sets
/// `PRAGMA query_only` (technical considerations: "the materialise path
/// opens its own connection without the pragma"), inside one transaction
/// with the `_mcphost_handles` meta row, the quota/eviction check
/// (requirement 4/AC5), and the `dataset-summary.v1` build -- a failure at
/// any point rolls the whole transaction back, so a refused materialise
/// (AC5's oversized case, a query-time-cap hit) never leaves a stray
/// table or meta row behind.
fn materialize_handle_sync(conn: &Connection, sql: &str, p: MaterializeParams<'_>) -> Result<Value, AppError> {
    let MaterializeParams { ttl_s, now, handle_bytes_max, plan_name } = p;
    {
        let stmt = conn.prepare(sql)?;
        if !stmt.readonly() {
            return Err(query_rejected("statement is not read-only"));
        }
    }

    let interrupt = conn.get_interrupt_handle();
    std::thread::spawn(move || {
        std::thread::sleep(QUERY_TIME_CAP);
        interrupt.interrupt();
    });

    let handle = format!("{HANDLE_PREFIX}{}", generate_handle_id());

    conn.execute("BEGIN IMMEDIATE", [])?;
    let outcome: Result<Value, AppError> = (|| {
        if let Err(e) = conn.execute(&format!("CREATE TABLE {} AS {sql}", quote_ident(&handle)), []) {
            if let rusqlite::Error::SqliteFailure(se, _) = &e
                && se.code == rusqlite::ErrorCode::OperationInterrupted
            {
                return Err(bound_exceeded("time_cap_s", QUERY_TIME_CAP.as_secs() as i64));
            }
            return Err(AppError::from(e));
        }

        let row_count = row_count_sync(conn, &handle)?;
        let bytes = handle_bytes_sync(conn, &handle)?;

        if bytes > handle_bytes_max {
            return Err(handle_quota_exceeded(plan_name, handle_bytes_max));
        }
        evict_lru_handles_until_fits_sync(conn, bytes, handle_bytes_max)?;

        conn.execute(
            &format!(
                "INSERT INTO {HANDLES_META_TABLE} \
                 (handle, created_unix, expires_unix, sql, bytes, row_count, last_used_unix) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"
            ),
            params![handle, now, now + ttl_s, sql, bytes, row_count, now],
        )?;

        dataset_summary_sync(conn, &handle, sql, bytes, now + ttl_s)
    })();

    match outcome {
        Ok(v) => {
            conn.execute("COMMIT", [])?;
            Ok(v)
        }
        Err(e) => {
            let _ = conn.execute("ROLLBACK", []);
            Err(e)
        }
    }
}

async fn table_query_materialize(
    state: &AppState,
    tenant: &Tenant,
    sql: &str,
    ttl_s: Option<i64>,
) -> Result<Value, AppError> {
    let ttl_s = ttl_s.unwrap_or(HANDLE_TTL_DEFAULT_S);
    if !(1..=HANDLE_TTL_MAX_S).contains(&ttl_s) {
        return Err(AppError::InvalidArgs(format!(
            "ttl_s: must be between 1 and {HANDLE_TTL_MAX_S}; got {ttl_s}"
        )));
    }
    validate_query_structure(sql)?;

    let plan = plan_of(state, &tenant.plan)?;
    let handle_bytes_max = plan.table_handle_bytes_max;
    let plan_name = tenant.plan.clone();

    let path = tenant_db_path(state, tenant.id);
    let sql_owned = sql.to_string();
    let now = crate::state::now_unix();
    with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
        materialize_handle_sync(
            conn,
            &sql_owned,
            MaterializeParams { ttl_s, now, handle_bytes_max, plan_name: &plan_name },
        )
    })
    .await
}

/// requirement 3/AC2/AC3/AC7/AC11: every later `host.table.query` that
/// doesn't ask for a new handle -- unchanged structural read-only check,
/// plus (only when `sql` names a `hdl_` table) a live-handle check ahead
/// of running it (`handle_not_found` rather than a generic "no such
/// table") and a `last_used_unix` bump after (requirement 4's eviction
/// order).
///
/// Rebase note (run 368, hand-merged with the independently-landed
/// PRD-mcphost-table-context-and-sql-passthrough's query log; re-merged
/// again here with the independently-landed PRD-mcphost-query-diagnosis):
/// [`validate_query_structure`] and [`referenced_table_names`] both run
/// *inside* the connection closure now (rather than ahead of it) so a
/// structural refusal or a `handle_not_found` gets exactly one
/// [`log_query`] row too, same as [`table_query_materialize`]'s sibling
/// path did before this PRD split it in two -- requirement 2/3's own
/// "every call, whether it returns rows or is refused, gets one row"
/// still holds for this (non-materialising) path. The diagnosis pipeline
/// ([`QueryDiagContext`]/[`diagnose_sync`]) that PRD-mcphost-query-diagnosis
/// added to the pre-split `table_query` carries over unchanged, now run
/// from here instead. Materialising calls (`table_query_materialize`) are
/// not logged here -- out of this PRD's own scope, left for the
/// query-log PRD to pick up if it wants handle creation logged too.
async fn table_query_select(
    state: &AppState,
    tenant: &Tenant,
    sql: &str,
    end_user: Option<&EndUser>,
) -> Result<Value, AppError> {
    // PRD-mcphost-query-diagnosis requirement 1/2: the AST parse is pure
    // (no IO); the annotation read is the one piece of diagnosis context
    // that genuinely needs the main database, fetched here (async) before
    // the per-tenant connection's sync/blocking closure runs.
    let extracted = crate::query_diag::extract_from_ast(sql);
    let column_annotation_candidates = if let Some(table) = &extracted.from_table {
        let annotations = state
            .db
            .list_table_model_annotations(tenant.id, table.clone())
            .await
            .unwrap_or_default();
        annotations
            .iter()
            .filter(|a| !a.column_name.is_empty() && a.key == "description")
            .flat_map(|a| crate::query_diag::annotation_word_candidates(&a.column_name, &a.value))
            .collect()
    } else {
        Vec::new()
    };
    let ctx = QueryDiagContext {
        from_table: extracted.from_table,
        where_equalities: extracted.where_equalities,
        column_annotation_candidates,
    };

    let path = tenant_db_path(state, tenant.id);
    let now = crate::state::now_unix();

    // PRD-mcphost-row-policy requirement 1/4: a structural refusal (not a
    // SELECT/CTE, multiple statements, a parse error) never reaches a
    // `with_tenant_conn` closure below, but must still log exactly one row
    // for the attempt like every other outcome does -- same reasoning as
    // the `Err(e)` arms inside each access branch below, just with nothing
    // to time (there's no query to run). Parsed once, here, since the
    // row-policy rewrite below needs the AST, not just a yes/no validation.
    let mut query = match validate_query_structure(sql) {
        Ok(query) => query,
        Err(e) => {
            let sql_for_log = sql.to_string();
            let e_for_log = e.clone();
            let log_outcome: Result<(), AppError> =
                with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
                    log_query(
                        conn,
                        &sql_for_log,
                        QueryLogOutcome {
                            duration_ms: 0,
                            row_count: None,
                            sample_hash: None,
                            error: Some(&e_for_log),
                            diagnosis: None,
                            hint: None,
                            result_bytes: None,
                            est_tokens: None,
                        },
                    );
                    Ok(())
                })
                .await;
            if let Err(log_err) = log_outcome {
                tracing::warn!(error = %log_err, "failed to log a structurally-refused host.table.query attempt");
            }
            return Err(e);
        }
    };

    let access = rowpolicy::resolve_access(state, tenant.id, end_user).await?;
    match access {
        rowpolicy::Access::Unrestricted => {
            let sql_owned = sql.to_string();
            let rows = with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
                let started = std::time::Instant::now();
                let mut referenced_handles: Vec<String> = Vec::new();
                let result: Result<Vec<Value>, AppError> = (|| {
                    referenced_handles = referenced_table_names(&sql_owned)?
                        .into_iter()
                        .filter(|n| n.starts_with(HANDLE_PREFIX))
                        .collect();
                    for h in &referenced_handles {
                        let expires: Option<i64> = conn
                            .query_row(
                                &format!("SELECT expires_unix FROM {HANDLES_META_TABLE} WHERE handle = ?1"),
                                params![h],
                                |r| r.get(0),
                            )
                            .optional()?;
                        match expires {
                            Some(exp) if exp > now => {}
                            _ => return Err(handle_not_found(h)),
                        }
                    }
                    run_query_sync(conn, &sql_owned)
                })();
                let duration_ms = started.elapsed().as_millis() as i64;
                // `run_query_sync` leaves this connection's own `query_only`
                // pragma ON for the rest of its life -- turn it back off before
                // the log write below and the `last_used_unix` bookkeeping UPDATE
                // (requirement 4's eviction order), neither of which is part of
                // the caller's own read-only query.
                let _ = conn.pragma_update(None, "query_only", "OFF");
                let (diagnosis, hint) = diagnose_sync(conn, &ctx, &result);
                let diagnosis_str = diagnosis.as_ref().map(|v| v.to_string());
                match &result {
                    Ok(rows) => {
                        // requirement 3's 5ms p95 logging-overhead budget (shared
                        // with the dependency PRD's own log-write budget) means
                        // this must never serialize the result twice: the common
                        // case (<=20 rows, [`sample_hash_of`]'s own truncation
                        // bound) reuses this one serialization for both the
                        // footprint byte count and the drift sample hash; only a
                        // result over 20 rows (up to [`ROW_CAP`]) pays a second,
                        // smaller serialization of just its first 20.
                        let serialized = serde_json::to_string(rows).unwrap_or_default();
                        let bytes = serialized.len() as i64;
                        // P1 requirement 7: `mqo-session-footprint-meter`'s
                        // `tokens_from_chars` default -- `ceil(bytes / 4)`. `i64`'s
                        // `div_ceil` is unstable for signed integers; `bytes` is
                        // never negative (a serialized length), so the `(n + 3) / 4`
                        // idiom is exact and needs no feature gate.
                        let est_tokens = (bytes + 3) / 4;
                        let sample_hash = if rows.len() <= 20 {
                            crate::billing::sha256_hex(serialized.as_bytes())
                        } else {
                            sample_hash_of(rows)
                        };
                        log_query(
                            conn,
                            &sql_owned,
                            QueryLogOutcome {
                                duration_ms,
                                row_count: Some(rows.len() as i64),
                                sample_hash: Some(&sample_hash),
                                error: None,
                                diagnosis: diagnosis_str.as_deref(),
                                hint: hint.as_deref(),
                                result_bytes: Some(bytes),
                                est_tokens: Some(est_tokens),
                            },
                        )
                    }
                    Err(e) => log_query(
                        conn,
                        &sql_owned,
                        QueryLogOutcome {
                            duration_ms,
                            row_count: None,
                            sample_hash: None,
                            error: Some(e),
                            diagnosis: diagnosis_str.as_deref(),
                            hint: hint.as_deref(),
                            result_bytes: None,
                            est_tokens: None,
                        },
                    ),
                }
                let out = result?;
                if !referenced_handles.is_empty() {
                    for h in &referenced_handles {
                        conn.execute(
                            &format!("UPDATE {HANDLES_META_TABLE} SET last_used_unix = ?1 WHERE handle = ?2"),
                            params![now, h],
                        )?;
                    }
                }
                Ok(out)
            })
            .await?;
            Ok(json!({"rows": rows}))
        }
        rowpolicy::Access::Restricted(rctx) => {
            // PRD-mcphost-row-policy requirement 4: rewritten via the AST
            // (never string concatenation) -- see src/rowpolicy/rewrite.rs.
            // A handle-prefixed table is never policied (handles are
            // tenant-key-only result caches, PRD-mcphost-result-handles),
            // so there is no handle-reference check to merge in here.
            let mut table_names = rowpolicy::rewrite::referenced_table_names(&query);
            table_names.sort();

            let mut compiled = std::collections::HashMap::new();
            for name in &table_names {
                let policy = rowpolicy::load_table_policy(state, tenant.id, name).await?;
                compiled.insert(name.clone(), rowpolicy::policy::compile(&policy, &rctx));
            }

            let outcome = rowpolicy::rewrite::apply(&mut query, &compiled)?;
            let rewritten_sql = outcome.sql;
            let bindings = outcome.bindings;

            let primary =
                table_names.first().and_then(|name| compiled.get(name).map(|c| (name.clone(), c.clone())));

            let sql_owned = sql.to_string();
            let (rows, withheld_count) = with_tenant_conn(
                path,
                state.db.cfg(),
                state.db.counters_handle(),
                move |conn| {
                    let started = std::time::Instant::now();
                    let result = run_query_sync_with_bindings(conn, &rewritten_sql, &bindings);
                    let duration_ms = started.elapsed().as_millis() as i64;
                    // same "turn query_only back off before the log write"
                    // reasoning as the Unrestricted branch above.
                    let _ = conn.pragma_update(None, "query_only", "OFF");
                    let (diagnosis, hint) = diagnose_sync(conn, &ctx, &result);
                    let diagnosis_str = diagnosis.as_ref().map(|v| v.to_string());
                    let rows = match result {
                        Ok(rows) => rows,
                        Err(e) => {
                            log_query(
                                conn,
                                &sql_owned,
                                QueryLogOutcome {
                                    duration_ms,
                                    row_count: None,
                                    sample_hash: None,
                                    error: Some(&e),
                                    diagnosis: diagnosis_str.as_deref(),
                                    hint: hint.as_deref(),
                                    result_bytes: None,
                                    est_tokens: None,
                                },
                            );
                            return Err(e);
                        }
                    };
                    let withheld = match &primary {
                        Some((table, compiled)) => withheld_count_sync(conn, table, compiled)?,
                        None => None,
                    };
                    let serialized = serde_json::to_string(&rows).unwrap_or_default();
                    let bytes = serialized.len() as i64;
                    let est_tokens = (bytes + 3) / 4;
                    let sample_hash = if rows.len() <= 20 {
                        crate::billing::sha256_hex(serialized.as_bytes())
                    } else {
                        sample_hash_of(&rows)
                    };
                    log_query(
                        conn,
                        &sql_owned,
                        QueryLogOutcome {
                            duration_ms,
                            row_count: Some(rows.len() as i64),
                            sample_hash: Some(&sample_hash),
                            error: None,
                            diagnosis: diagnosis_str.as_deref(),
                            hint: hint.as_deref(),
                            result_bytes: Some(bytes),
                            est_tokens: Some(est_tokens),
                        },
                    );
                    Ok((rows, withheld))
                },
            )
            .await?;

            if let Some(primary_name) = table_names.first()
                && let Some(primary_compiled) = compiled.get(primary_name)
            {
                state
                    .db
                    .audit_chain_append(
                        tenant.id,
                        rctx.subject.clone(),
                        "sql".to_string(),
                        primary_compiled.policy_hash.clone(),
                        primary_compiled.rls_predicate.clone(),
                        rows.len() as i64,
                        withheld_count,
                        crate::state::now_unix(),
                        crate::state::new_ulid(),
                    )
                    .await?;
            }

            Ok(json!({"rows": rows}))
        }
    }
}

pub async fn table_query(
    state: &AppState,
    tenant: &Tenant,
    args: &Value,
    end_user: Option<&EndUser>,
) -> Result<Value, AppError> {
    let sql = arg_str(args, "sql")?;
    let want_handle = args.get("handle").and_then(Value::as_bool).unwrap_or(false);
    if want_handle {
        let ttl_s = args.get("ttl_s").and_then(Value::as_i64);
        return table_query_materialize(state, tenant, &sql, ttl_s).await;
    }
    table_query_select(state, tenant, &sql, end_user).await
}

/// requirement 5: `host.table.handles()` -- every live handle, newest
/// first, with the fields requirement 5 names. Expired-but-not-yet-ticked
/// handles are filtered out here too (AC4: gone from this listing well
/// before the tick necessarily gets to them, since the tick's own cadence
/// is the *upper* bound on when the table itself is dropped, not a lower
/// one on when callers stop seeing it as live).
pub async fn table_handles(state: &AppState, tenant: &Tenant, _args: &Value) -> Result<Value, AppError> {
    let path = tenant_db_path(state, tenant.id);
    let now = crate::state::now_unix();
    let handles = with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
        let mut stmt = conn.prepare(&format!(
            "SELECT handle, row_count, bytes, created_unix, expires_unix, last_used_unix, sql \
             FROM {HANDLES_META_TABLE} WHERE expires_unix > ?1 ORDER BY created_unix DESC, rowid DESC"
        ))?;
        let rows = stmt
            .query_map(params![now], |r| {
                Ok(json!({
                    "handle": r.get::<_, String>(0)?,
                    "row_count": r.get::<_, i64>(1)?,
                    "bytes": r.get::<_, i64>(2)?,
                    "created_unix": r.get::<_, i64>(3)?,
                    "expires_unix": r.get::<_, i64>(4)?,
                    "last_used_unix": r.get::<_, i64>(5)?,
                    "derived_from": r.get::<_, String>(6)?,
                }))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
    .await?;
    Ok(json!({"handles": handles}))
}

/// requirement 5: `host.table.handle_drop(handle)` -- drops one handle's
/// table and meta row; a handle that doesn't exist (or already expired) is
/// a no-op (`dropped: false`), not an error -- same "idempotent drop"
/// shape [`table_drop`] already gives a declared table.
pub async fn table_handle_drop(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let handle = arg_str(args, "handle")?;
    let path = tenant_db_path(state, tenant.id);
    let handle_for_conn = handle.clone();
    let dropped = with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
        let existing: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM {HANDLES_META_TABLE} WHERE handle = ?1"),
            params![handle_for_conn],
            |r| r.get(0),
        )?;
        if existing == 0 {
            return Ok(false);
        }
        drop_handle_row_sync(conn, &handle_for_conn)?;
        Ok(true)
    })
    .await?;
    Ok(json!({"handle": handle, "dropped": dropped}))
}

/// requirement 4/AC4: drops every handle whose `expires_unix` has passed,
/// across every tenant -- same "list every tenant, best-effort per
/// tenant" shape `tables_model::tick_once` uses, exposed directly so a
/// test can drive one deterministic cycle instead of waiting on the real
/// cadence (same convention as `bans::tick_once`/`tables_model::tick_once`).
pub async fn tick_once(state: &AppState) -> Result<(), AppError> {
    let tenants = state.db.list_tenants().await?;
    let now = crate::state::now_unix();
    for tenant in tenants {
        let path = tenant_db_path(state, tenant.id);
        if !path.exists() {
            continue;
        }
        let result = with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
            let expired: Vec<String> = {
                let mut stmt = conn.prepare(&format!(
                    "SELECT handle FROM {HANDLES_META_TABLE} WHERE expires_unix <= ?1"
                ))?;
                stmt.query_map(params![now], |r| r.get(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            };
            for handle in expired {
                drop_handle_row_sync(conn, &handle)?;
            }
            Ok(())
        })
        .await;
        if let Err(e) = result {
            tracing::warn!(error = %e, tenant_id = tenant.id, "handle expiry tick failed for tenant");
        }
    }
    Ok(())
}

/// P1 requirement 7/AC8: every row of `handle`, column names in
/// declaration order, for `export.rs`'s `handle_export` to write as CSV --
/// unlike [`run_query_sync`], never bounded by [`ROW_CAP`] (a handle
/// export's whole point is getting every row of a result that was itself
/// materialised specifically to get around that cap), bumping
/// `last_used_unix` same as any other reference to the handle
/// (requirement 4's eviction order).
pub async fn handle_export_rows(
    state: &AppState,
    tenant: &Tenant,
    handle: &str,
) -> Result<(Vec<String>, Vec<Value>), AppError> {
    let path = tenant_db_path(state, tenant.id);
    let handle_owned = handle.to_string();
    let now = crate::state::now_unix();
    with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
        let expires: Option<i64> = conn
            .query_row(
                &format!("SELECT expires_unix FROM {HANDLES_META_TABLE} WHERE handle = ?1"),
                params![handle_owned],
                |r| r.get(0),
            )
            .optional()?;
        match expires {
            Some(exp) if exp > now => {}
            _ => return Err(handle_not_found(&handle_owned)),
        }
        let column_names: Vec<String> = {
            let stmt = conn.prepare(&format!("SELECT * FROM {} LIMIT 0", quote_ident(&handle_owned)))?;
            stmt.column_names().into_iter().map(str::to_string).collect()
        };
        let rows = select_all_sync(conn, &format!("SELECT * FROM {}", quote_ident(&handle_owned)))?;
        conn.execute(
            &format!("UPDATE {HANDLES_META_TABLE} SET last_used_unix = ?1 WHERE handle = ?2"),
            params![now, handle_owned],
        )?;
        Ok((column_names, rows))
    })
    .await
}

/// requirement 4: the background tick, started once alongside this
/// crate's other tick loops (see `main.rs`) -- same spawn/sleep-loop shape
/// as `tables_model::spawn_tick`. A 30s cadence keeps every expired
/// handle's drop well inside the 60s `expires_unix` bound AC4 names.
pub fn spawn_tick(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(HANDLE_TICK_INTERVAL_SECS)).await;
            if let Err(e) = tick_once(&state).await {
                tracing::warn!(error = %e, "handle expiry tick failed");
            }
        }
    })
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
/// PRD-mcphost-table-context-and-sql-passthrough requirement 1/AC1-3: one
/// table's columns, types, row count, byte count, and -- whenever a
/// `description` annotation exists ([`crate::tables_model::model_set`]) --
/// that text at table level and/or per column. Only `description` is
/// merged in (not `role`/`unit`/`hidden`, which `host.table.describe`
/// already surfaces): a column with no `description` annotation keeps its
/// pre-change bare-string entry (`{"col": "text"}`), and a table with no
/// annotations at all returns the identical pre-change shape (AC3).
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
    let mut columns: Map<String, Value> = schema
        .columns
        .iter()
        .map(|(k, v)| (k.clone(), json!(v.as_str())))
        .collect();

    let annotations = state.db.list_table_model_annotations(tenant.id, table.clone()).await?;
    let mut table_description: Option<String> = None;
    for ann in &annotations {
        if ann.key != "description" {
            continue;
        }
        if ann.column_name.is_empty() {
            table_description = Some(ann.value.clone());
        } else if let Some(Value::String(type_str)) = columns.get(&ann.column_name).cloned() {
            columns.insert(
                ann.column_name.clone(),
                json!({"type": type_str, "description": ann.value}),
            );
        }
    }

    let bytes_used = std::fs::metadata(&path_for_size).map(|m| m.len() as i64).unwrap_or(0);
    let mut out = Map::new();
    out.insert("table".to_string(), json!(table));
    out.insert("columns".to_string(), Value::Object(columns));
    out.insert("rows".to_string(), json!(row_count));
    out.insert("bytes_used".to_string(), json!(bytes_used));
    if let Some(description) = table_description {
        out.insert("description".to_string(), json!(description));
    }
    Ok(Value::Object(out))
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
        let session_bindings = crate::session_bind::SessionBindings::new();
        AppState {
            db,
            kinds: crate::kinds::KindRegistry::with_builtin(),
            secrets: crate::secrets::SecretBox::from_passphrase("test-secret-key"),
            admin_key: Some("test-admin-key".to_string()),
            public_url: "http://127.0.0.1:0".to_string(),
            alt_public_urls: Vec::new(),
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
            authz_key: crate::authz::AuthzSigningKey::load_or_generate(dir).expect("authz signing key"),
            cimd_cache: crate::authz::CimdCache::new(),
            alerts: crate::alerts::AlertRegistry::new(),
            contention_tracker: crate::alerts::ContentionTracker::new(),
            alert_config: crate::alerts::AlertConfig::default(),
            alert_quota_trips: crate::alerts::QuotaTripTracker::new(),
            status_probe_override: crate::statusfeed::ProbeOverrides::new(),
            fleet_ips: crate::state::FleetIps::empty(),
            end_user_activity: Default::default(),
            oauth_healthz_cache: Default::default(),
            verified_client_ids: crate::state::VerifiedClientIds::empty(),
            session_bindings: session_bindings.clone(),
            invite_hints: crate::invites::InviteHintTracker::new(),
            lineage_cache: crate::lineage::new_cache(),
            lineage_trace_pages: crate::lineage::new_trace_page_cache(),
            public_url_sync_deadline: crate::state::PUBLIC_URL_SYNC_DEADLINE,
            url_rate_limiter: crate::hooks::EventRateLimiter::new(),
            implicit_signup_memory: crate::session_bind::SessionBindings::new_sharing_secret(&session_bindings),
            tenant_key_arg_memory: crate::session_bind::SessionBindings::new_sharing_secret(&session_bindings),
            reuse_session_tenant: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
            None,
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

        let result = table_query(&state, &t, &json!({"sql": "SELECT * FROM metrics"}), None)
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
            None,
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

        let err = table_query(&state, &t, &json!({"sql": "SELECT * FROM big"}), None)
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
