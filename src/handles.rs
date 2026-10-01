//! PRD-mcphost-result-handles: `host.table.query(sql, handle: true, ttl_s?)`
//! materialises a SELECT's result as a genuine table inside the tenant's
//! own `tables/<tenant_id>.db` file (the same per-tenant SQLite store
//! `tables.rs` already owns -- see that module's doc header for why), named
//! `hdl_<id>`, with no `tables::ROW_CAP` -- only `tables::QUERY_TIME_CAP`
//! bounds it. The call returns a bounded `dataset-summary.v1`: a `sample`
//! of at most 20 rows and per-column `stats` computed by SQL over *every*
//! row of the handle, never the sample. Later SQL can `SELECT ... FROM
//! hdl_<id>` through the ordinary `host.table.query` path (`tables.rs`'s
//! three-layer read-only enforcement applies unchanged -- a handle is just
//! another table in that same file), which is also where a handle's
//! `last_used_unix` gets bumped (the eviction order's own input) and where
//! a reference to a missing/expired handle is turned into `handle_not_found`
//! rather than SQLite's bare "no such table".
//!
//! A handle expires on its own (`ttl_s`, default 3,600s, max 86,400s) and
//! counts against its own per-tenant byte quota
//! (`Plan::table_handle_bytes_max`), separate from `Plan::table_bytes_max`
//! (Technical considerations: a handle is a transient derived result, not
//! a table a tenant chose to keep). [`tick_once`] reaps expired handles
//! across every tenant within [`TICK_INTERVAL_SECS`] of expiry;
//! [`materialize`] itself also reaps its own tenant's expired handles and,
//! if still over quota, evicts the least-recently-queried ones first, all
//! inside the one transaction that creates the new handle -- so "nothing is
//! created" (requirement 4) when even that isn't enough.
//!
//! `bytes` (both the quota input and the `dataset-summary.v1` field) is a
//! SQL-computed estimate -- `SUM` of each column's `CAST(... AS TEXT)`
//! length, the same "a cheap proxy stands in for real storage bytes"
//! approximation `tables.rs::table_append`'s own `table_bytes_max` check
//! already makes for the KV/table byte quotas elsewhere in this crate --
//! rather than SQLite's `dbstat` virtual table (not compiled into this
//! crate's bundled SQLite) or a post-commit file-size delta (which
//! wouldn't shrink back down on an eviction's `DROP TABLE` without a
//! `VACUUM` this module never runs).

use std::collections::BTreeSet;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params, types::ValueRef};
use serde_json::{Map, Value, json};

use crate::db::Tenant;
use crate::errors::AppError;
use crate::state::AppState;
use crate::tables::{self, quote_ident, value_ref_to_json};

/// requirement 1/AC6: the reserved table-name prefix every materialised
/// handle's own SQLite table (and its `_mcphost_handles` meta row) is
/// named under -- `tables::table_create`/`table_drop` both refuse a
/// declared table starting with it.
pub(crate) const HANDLE_PREFIX: &str = "hdl_";

const HANDLES_META_TABLE: &str = "_mcphost_handles";

/// requirement 4: "Default `ttl_s` 3,600, maximum 86,400".
const DEFAULT_TTL_S: i64 = 3_600;
const MAX_TTL_S: i64 = 86_400;

/// requirement 2: "a sample, and stats over all rows" -- `sample_cap`.
const SAMPLE_CAP: i64 = 20;

/// requirement 5: how many of a column's most frequent non-null values
/// `stats.<col>.top_k` carries.
const TOP_K: i64 = 5;

/// requirement 4: "An expired handle is dropped by the tables tick within
/// 60s of expiry" -- a 30s cadence bounds that with headroom, the same
/// "the tick's own period bounds the staleness window" reasoning
/// `tables_model::TICK_INTERVAL_SECS`'s doc comment already uses for its
/// own (different) 30s bound.
const TICK_INTERVAL_SECS: u64 = 30;

/// Lowercase Crockford base32 (excludes i, l, o, u) -- the same alphabet
/// [`crate::state::new_ulid`] uses, lowercased, since a handle name is
/// spliced into SQL identifiers a caller reads back in `sample`/tool specs
/// where lowercase reads more like an ordinary table name than a ULID does.
const HANDLE_ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// requirement 1: "`<id>` is 12 lowercase base32 characters" -- 60 bits of
/// randomness, collision-resistant enough for a per-tenant, 24-hour-lived
/// name (the same "generate and sort/compare, never parse back" stance
/// `new_ulid` already takes, minus the sortable timestamp prefix this
/// doesn't need).
fn new_handle_id() -> String {
    use rand::RngCore;
    let mut rng = rand::thread_rng();
    let mut buf = [0u8; 8];
    rng.fill_bytes(&mut buf);
    let mut acc: u64 = 0;
    for b in buf {
        acc = (acc << 8) | b as u64;
    }
    let mut out = String::with_capacity(12);
    for i in (0..12).rev() {
        let shift = i * 5;
        let idx = ((acc >> shift) & 0x1f) as usize;
        out.push(HANDLE_ALPHABET[idx] as char);
    }
    out
}

pub(crate) fn handle_not_found(handle: &str) -> AppError {
    AppError::Structured {
        code: "handle_not_found",
        message: format!("no handle '{handle}' found for this tenant"),
        data: json!({"handle": handle}),
    }
}

fn handle_quota_exceeded(bytes: i64, cap: i64) -> AppError {
    AppError::Structured {
        code: "handle_quota_exceeded",
        message: format!(
            "materialized result is {bytes} bytes, over this plan's table_handle_bytes_max cap of {cap} bytes"
        ),
        data: json!({"limit": {"name": "table_handle_bytes_max", "value": cap}, "bytes": bytes}),
    }
}

/// Bootstraps [`HANDLES_META_TABLE`] -- called from `tables::open_conn` on
/// every connection this crate opens to a tenant's table file (not just the
/// ones this module itself opens), so `host.table.query`'s handle check
/// never has to special-case "the table doesn't exist yet" as anything
/// other than "zero rows".
pub(crate) fn ensure_meta_table_sync(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {HANDLES_META_TABLE} (
            handle TEXT PRIMARY KEY,
            row_count INTEGER NOT NULL,
            bytes INTEGER NOT NULL,
            created_unix INTEGER NOT NULL,
            expires_unix INTEGER NOT NULL,
            last_used_unix INTEGER NOT NULL,
            derived_from TEXT NOT NULL
        );"
    ))?;
    Ok(())
}

/// requirement 3/AC3/AC11: every `hdl_<id>`-looking relation `sql`
/// references, lowercased and deduplicated -- a structural walk of the
/// parsed statement (`sqlparser`'s `visitor` feature, over CTEs, joins and
/// subqueries alike), not a string search, the same "don't hand-roll what
/// the parser already gets right" stance `tables::parse_single_select`
/// already takes. A name that doesn't start with [`HANDLE_PREFIX`] is
/// never a handle -- `tables::table_create`/`table_drop` both refuse that
/// prefix for a declared table, so any such reference can only mean a
/// handle, real or mistyped.
pub(crate) fn extract_handle_names(statements: &[sqlparser::ast::Statement]) -> Vec<String> {
    use sqlparser::ast::visit_relations;
    use std::ops::ControlFlow;

    let mut names = BTreeSet::new();
    let _ = visit_relations(&statements.to_vec(), |relation| {
        let name = relation.to_string().to_lowercase();
        if name.starts_with(HANDLE_PREFIX) {
            names.insert(name);
        }
        ControlFlow::<()>::Continue(())
    });
    names.into_iter().collect()
}

fn handle_is_live_sync(conn: &Connection, handle: &str, now: i64) -> Result<bool, AppError> {
    let expires: Option<i64> = conn
        .query_row(
            &format!("SELECT expires_unix FROM {HANDLES_META_TABLE} WHERE handle = ?1"),
            params![handle],
            |r| r.get(0),
        )
        .optional()?;
    Ok(matches!(expires, Some(e) if e > now))
}

/// requirement 3/AC3/AC11: called on the exact connection `table_query`'s
/// own `run_query_sync` is about to run on, before it runs -- so a missing
/// or just-expired handle is refused `handle_not_found` naming it, rather
/// than falling through to SQLite's own generic "no such table" (which
/// `table_not_found`-shaped callers get, but a handle reference should not:
/// the PRD pins the wire code to exactly `handle_not_found`).
pub(crate) fn check_handles_live_sync(conn: &Connection, names: &[String]) -> Result<(), AppError> {
    let now = crate::state::now_unix();
    for name in names {
        if !handle_is_live_sync(conn, name, now)? {
            return Err(handle_not_found(name));
        }
    }
    Ok(())
}

/// requirement 4 (Eviction order): bumps every referenced handle's
/// `last_used_unix` after a successful query -- called on the same
/// connection, right after `run_query_sync` returns, so there is no
/// interleaving with a concurrent call against this same tenant file to
/// race against.
pub(crate) fn bump_last_used_sync(conn: &Connection, names: &[String]) -> Result<(), AppError> {
    if names.is_empty() {
        return Ok(());
    }
    let now = crate::state::now_unix();
    for name in names {
        conn.execute(
            &format!("UPDATE {HANDLES_META_TABLE} SET last_used_unix = ?1 WHERE handle = ?2"),
            params![now, name],
        )?;
    }
    Ok(())
}

/// requirement 4: drops every handle (table + meta row) whose
/// `expires_unix` is at or before `now` -- shared by [`materialize`]'s own
/// opportunistic reap (so a tenant's quota accounting never counts an
/// already-expired handle against a fresh materialisation) and [`tick_once`]'s
/// cross-tenant sweep.
fn reap_expired_sync(conn: &Connection, now: i64) -> Result<(), AppError> {
    let mut stmt = conn.prepare(&format!("SELECT handle FROM {HANDLES_META_TABLE} WHERE expires_unix <= ?1"))?;
    let names: Vec<String> = stmt.query_map(params![now], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    drop(stmt);
    for handle in names {
        conn.execute(&format!("DROP TABLE IF EXISTS {}", quote_ident(&handle)), [])?;
        conn.execute(&format!("DELETE FROM {HANDLES_META_TABLE} WHERE handle = ?1"), params![handle])?;
    }
    Ok(())
}

/// requirement 4: drops the least-recently-queried live handles, oldest
/// `last_used_unix` first, until at least `need_to_free` bytes' worth (by
/// each evicted handle's own stored `bytes`) have been removed -- called
/// only after [`reap_expired_sync`] and only when the new handle's own
/// bytes still don't fit alongside what's already live.
fn evict_lru_sync(conn: &Connection, need_to_free: i64) -> Result<(), AppError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT handle, bytes FROM {HANDLES_META_TABLE} ORDER BY last_used_unix ASC"
    ))?;
    let mut rows = stmt.query([])?;
    let mut to_drop = Vec::new();
    let mut freed = 0i64;
    while freed < need_to_free {
        match rows.next()? {
            Some(row) => {
                let handle: String = row.get(0)?;
                let bytes: i64 = row.get(1)?;
                freed += bytes;
                to_drop.push(handle);
            }
            None => break,
        }
    }
    drop(rows);
    drop(stmt);
    for handle in to_drop {
        conn.execute(&format!("DROP TABLE IF EXISTS {}", quote_ident(&handle)), [])?;
        conn.execute(&format!("DELETE FROM {HANDLES_META_TABLE} WHERE handle = ?1"), params![handle])?;
    }
    Ok(())
}

fn parse_ttl_s(args: &Value) -> Result<i64, AppError> {
    match args.get("ttl_s") {
        None | Some(Value::Null) => Ok(DEFAULT_TTL_S),
        Some(v) => {
            let n = v
                .as_i64()
                .ok_or_else(|| AppError::InvalidArgs("ttl_s: must be an integer".to_string()))?;
            if !(1..=MAX_TTL_S).contains(&n) {
                return Err(AppError::InvalidArgs(format!(
                    "ttl_s: must be between 1 and {MAX_TTL_S}; got {n}"
                )));
            }
            Ok(n)
        }
    }
}

/// Every declared column of the just-created handle table, in declaration
/// order: `(name, dtype, nullable)`. `PRAGMA table_info` gives the name
/// reliably but, for a `CREATE TABLE ... AS SELECT` table, no declared
/// type (CTAS assigns no column type/affinity) -- so `dtype` is inferred
/// from one sample stored value's own `typeof()` instead (requirement 2:
/// "`dtype`" in the `dataset-summary.v1` columns array), defaulting to
/// `"text"` for an all-null column (AC12: an empty handle reports every
/// column this way too, since `typeof()` never returns a row then either).
fn load_handle_columns_sync(conn: &Connection, handle: &str) -> Result<Vec<(String, String, bool)>, AppError> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({})", quote_ident(handle)))?;
    let names: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<_>>()?;
    drop(stmt);

    let table_ident = quote_ident(handle);
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let col_ident = quote_ident(&name);
        let dtype: Option<String> = conn
            .query_row(
                &format!("SELECT typeof({col_ident}) FROM {table_ident} WHERE {col_ident} IS NOT NULL LIMIT 1"),
                [],
                |r| r.get(0),
            )
            .optional()?;
        let dtype = match dtype.as_deref() {
            Some("integer") => "integer",
            Some("real") => "real",
            // `text`/`blob`/`null` (no non-null sample row) all degrade to
            // `text` -- the same "describe a BLOB, never error the call
            // over it" stance `tables::value_ref_to_json` already takes
            // (no `host.table.create` column is ever declared BLOB; a
            // CTAS result over a caller's own SQL expression could still
            // produce one).
            _ => "text",
        }
        .to_string();
        let nullable: bool = conn.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM {table_ident} WHERE {col_ident} IS NULL)"),
            [],
            |r| r.get(0),
        )?;
        out.push((name, dtype, nullable));
    }
    Ok(out)
}

/// requirement 2: "`sum` and `mean` only for numeric columns" -- `bytes` is
/// the SQL-computed proxy this module's doc header explains (`SUM` of each
/// column's text-cast length), used both for the `dataset-summary.v1`
/// field and the handle-bytes quota check in [`materialize_sync`].
fn estimate_bytes_sync(conn: &Connection, handle: &str, columns: &[(String, String, bool)]) -> Result<i64, AppError> {
    if columns.is_empty() {
        return Ok(0);
    }
    let expr = columns
        .iter()
        .map(|(name, _, _)| format!("LENGTH(COALESCE(CAST({} AS TEXT), ''))", quote_ident(name)))
        .collect::<Vec<_>>()
        .join(" + ");
    let bytes: Option<i64> =
        conn.query_row(&format!("SELECT SUM({expr}) FROM {}", quote_ident(handle)), [], |r| r.get(0))?;
    Ok(bytes.unwrap_or(0))
}

fn sample_rows_sync(conn: &Connection, handle: &str, limit: i64) -> Result<Vec<Value>, AppError> {
    let sql = format!("SELECT * FROM {} LIMIT {limit}", quote_ident(handle));
    let mut stmt = conn.prepare(&sql)?;
    let column_names: Vec<String> = stmt.column_names().into_iter().map(str::to_string).collect();
    let mut rows_out = Vec::new();
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let mut obj = Map::new();
        for (i, name) in column_names.iter().enumerate() {
            obj.insert(name.clone(), value_ref_to_json(row.get_ref(i)?));
        }
        rows_out.push(Value::Object(obj));
    }
    Ok(rows_out)
}

/// requirement 2/AC1: `min`/`max`/`distinct`/`top_k` for every column,
/// `sum`/`mean` added only for a numeric (`integer`/`real`) one -- each
/// computed by its own `SELECT` over the *whole* handle table (never the
/// `sample`), so a caller's "recompute `sum`/`distinct` by SQL" check
/// (AC1) always matches. AC12 (an empty handle): every aggregate here
/// degrades to SQL's own empty-set answer -- `MIN`/`MAX`/`SUM`/`AVG` are
/// `NULL`, `COUNT(DISTINCT ...)` is `0`, `top_k` is `[]`.
fn compute_stats_sync(conn: &Connection, handle: &str, columns: &[(String, String, bool)]) -> Result<Value, AppError> {
    let table_ident = quote_ident(handle);
    let mut stats = Map::new();
    for (name, dtype, _nullable) in columns {
        let col_ident = quote_ident(name);
        let (min_v, max_v) = conn.query_row(
            &format!("SELECT MIN({col_ident}), MAX({col_ident}) FROM {table_ident}"),
            [],
            |r| Ok((value_ref_to_json(r.get_ref(0)?), value_ref_to_json(r.get_ref(1)?))),
        )?;
        let distinct: i64 =
            conn.query_row(&format!("SELECT COUNT(DISTINCT {col_ident}) FROM {table_ident}"), [], |r| r.get(0))?;

        let mut entry = Map::new();
        entry.insert("min".to_string(), min_v);
        entry.insert("max".to_string(), max_v);
        entry.insert("distinct".to_string(), json!(distinct));

        if dtype == "integer" || dtype == "real" {
            let (sum_v, mean_v): (Option<f64>, Option<f64>) = conn.query_row(
                &format!("SELECT SUM({col_ident}), AVG({col_ident}) FROM {table_ident}"),
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            entry.insert("sum".to_string(), sum_v.map(|v| json!(v)).unwrap_or(Value::Null));
            entry.insert("mean".to_string(), mean_v.map(|v| json!(v)).unwrap_or(Value::Null));
        }

        let mut top_k_stmt = conn.prepare(&format!(
            "SELECT {col_ident} AS v, COUNT(*) AS c FROM {table_ident} WHERE {col_ident} IS NOT NULL \
             GROUP BY {col_ident} ORDER BY c DESC LIMIT {TOP_K}"
        ))?;
        let top_k: Vec<Value> = top_k_stmt
            .query_map([], |r| {
                Ok(json!({"value": value_ref_to_json(r.get_ref(0)?), "count": r.get::<_, i64>(1)?}))
            })?
            .collect::<rusqlite::Result<_>>()?;
        entry.insert("top_k".to_string(), json!(top_k));

        stats.insert(name.clone(), Value::Object(entry));
    }
    Ok(Value::Object(stats))
}

/// requirement 1/Technical considerations: runs `CREATE TABLE hdl_<id> AS
/// <sql>` plus every stat/sample query, the quota/eviction decision, and
/// the `_mcphost_handles` meta-row insert all inside one transaction --
/// rolled back whole on any failure (including the handle's own bytes
/// alone exceeding the quota, requirement 4's "nothing is created") so a
/// caller never sees a half-materialised handle.
#[allow(clippy::too_many_arguments)]
fn materialize_sync(
    conn: &Connection,
    handle: &str,
    sql: &str,
    ttl_s: i64,
    now: i64,
    bytes_max: i64,
) -> Result<Value, AppError> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let outcome: Result<Value, AppError> = (|| {
        // Technical considerations: "QUERY_TIME_CAP does [apply]" -- same
        // detached-thread-interrupt mechanism `tables::run_query_sync`
        // uses, bounding just the potentially-unbounded `CREATE TABLE AS`
        // scan (the aggregate/sample queries below run over the now-
        // bounded-size handle itself).
        let interrupt = conn.get_interrupt_handle();
        std::thread::spawn(move || {
            std::thread::sleep(tables::QUERY_TIME_CAP);
            interrupt.interrupt();
        });
        let create_sql = format!("CREATE TABLE {} AS {sql}", quote_ident(handle));
        match conn.execute(&create_sql, []) {
            Ok(_) => {}
            Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == rusqlite::ErrorCode::OperationInterrupted => {
                return Err(tables::bound_exceeded("time_cap_s", tables::QUERY_TIME_CAP.as_secs() as i64));
            }
            Err(e) => return Err(AppError::from(e)),
        }

        let row_count: i64 =
            conn.query_row(&format!("SELECT COUNT(*) FROM {}", quote_ident(handle)), [], |r| r.get(0))?;
        let columns = load_handle_columns_sync(conn, handle)?;
        let bytes = estimate_bytes_sync(conn, handle, &columns)?;

        // requirement 4: a stale handle this tenant hasn't been billed a
        // reap for yet must not count against the quota below, nor sit
        // around past the background tick for no reason.
        reap_expired_sync(conn, now)?;

        if bytes > bytes_max {
            return Err(handle_quota_exceeded(bytes, bytes_max));
        }
        let live_bytes: i64 =
            conn.query_row(&format!("SELECT COALESCE(SUM(bytes), 0) FROM {HANDLES_META_TABLE}"), [], |r| r.get(0))?;
        if live_bytes + bytes > bytes_max {
            evict_lru_sync(conn, live_bytes + bytes - bytes_max)?;
        }

        let expires_unix = now + ttl_s;
        conn.execute(
            &format!(
                "INSERT INTO {HANDLES_META_TABLE} \
                 (handle, row_count, bytes, created_unix, expires_unix, last_used_unix, derived_from) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"
            ),
            params![handle, row_count, bytes, now, expires_unix, now, sql],
        )?;

        let sample = sample_rows_sync(conn, handle, SAMPLE_CAP)?;
        let stats = compute_stats_sync(conn, handle, &columns)?;
        let columns_json: Vec<Value> = columns
            .iter()
            .map(|(name, dtype, nullable)| json!({"name": name, "dtype": dtype, "nullable": nullable}))
            .collect();

        Ok(json!({
            "handle": handle,
            "table": handle,
            "row_count": row_count,
            "columns": columns_json,
            "sample": sample,
            "sample_cap": SAMPLE_CAP,
            "stats": stats,
            "bytes": bytes,
            "expires_unix": expires_unix,
            "derived_from": sql,
        }))
    })();

    match outcome {
        Ok(v) => {
            conn.execute_batch("COMMIT")?;
            Ok(v)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// `host.table.query(sql, handle: true, ttl_s?)` -- requirement 1/2. Called
/// from `tables::table_query` once `sql` has already passed
/// `tables::parse_single_select`'s structural single-SELECT check.
pub async fn materialize(state: &AppState, tenant: &Tenant, sql: &str, args: &Value) -> Result<Value, AppError> {
    let ttl_s = parse_ttl_s(args)?;
    let plan = tables::plan_of(state, &tenant.plan)?;
    let bytes_max = plan.table_handle_bytes_max;

    let id = new_handle_id();
    let handle = format!("{HANDLE_PREFIX}{id}");
    let now = crate::state::now_unix();

    let path = tables::tenant_db_path(state, tenant.id);
    let sql_owned = sql.to_string();
    let handle_for_conn = handle.clone();
    tables::with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
        materialize_sync(conn, &handle_for_conn, &sql_owned, ttl_s, now, bytes_max)
    })
    .await
}

/// `host.table.handles()` -- requirement 5: live handles, newest first,
/// plus `bytes_used` (the sum of every listed handle's own `bytes` --
/// AC4's "bytes_used fell accordingly" once an expired one is reaped),
/// the handle-quota analogue of `tables::table_list`'s own file-size
/// `bytes_used` field for declared tables.
pub async fn handles_list(state: &AppState, tenant: &Tenant, _args: &Value) -> Result<Value, AppError> {
    let path = tables::tenant_db_path(state, tenant.id);
    let now = crate::state::now_unix();
    let handles = tables::with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
        // Keeps the listing honest against a tick that hasn't run yet --
        // never list a handle whose own TTL has already passed.
        reap_expired_sync(conn, now)?;
        let mut stmt = conn.prepare(&format!(
            "SELECT handle, row_count, bytes, created_unix, expires_unix, last_used_unix, derived_from \
             FROM {HANDLES_META_TABLE} ORDER BY created_unix DESC"
        ))?;
        let out = stmt
            .query_map([], |r| {
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
        Ok(out)
    })
    .await?;
    let bytes_used: i64 = handles.iter().filter_map(|h| h["bytes"].as_i64()).sum();
    Ok(json!({"handles": handles, "bytes_used": bytes_used}))
}

/// `host.table.handle_drop(handle)` -- requirement 5.
pub async fn handle_drop(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let handle = tables::arg_str(args, "handle")?;
    let path = tables::tenant_db_path(state, tenant.id);
    let handle_for_conn = handle.clone();
    let dropped = tables::with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
        let existing: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM {HANDLES_META_TABLE} WHERE handle = ?1"),
            params![handle_for_conn],
            |r| r.get(0),
        )?;
        if existing == 0 {
            return Ok(false);
        }
        conn.execute(&format!("DROP TABLE IF EXISTS {}", quote_ident(&handle_for_conn)), [])?;
        conn.execute(
            &format!("DELETE FROM {HANDLES_META_TABLE} WHERE handle = ?1"),
            params![handle_for_conn],
        )?;
        Ok(true)
    })
    .await?;
    Ok(json!({"handle": handle, "dropped": dropped}))
}

// ---- host.table.handle_export (P1 requirement 7) --------------------------

/// A distinct `runs.tool_name` from `export::EXPORT_TOOL_NAME` -- same
/// "the per-subject/per-kind 'one running at a time' check never collides
/// across kinds" reasoning `export::ENDUSER_EXPORT_TOOL_NAME`'s own doc
/// comment gives; `export::download` branches on this to pick `.csv`/
/// `text/csv` over the tar.gz export's own extension/content-type.
pub const HANDLE_EXPORT_TOOL_NAME: &str = "host.table.handle_export";

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn csv_field_string(v: ValueRef<'_>) -> String {
    match value_ref_to_json(v) {
        Value::Null => String::new(),
        Value::String(s) => s,
        other => other.to_string(),
    }
}

/// requirement 7/AC8: a header row plus one row per handle row, RFC
/// 4180-ish (CRLF line endings, quote-and-double-embedded-quote escaping)
/// -- hand-rolled rather than a new `csv` dependency, same "no new
/// dependency for a small, self-contained thing" call `export.rs`'s own
/// tar/gzip writer already makes for its archive layout.
fn build_csv_sync(conn: &Connection, handle: &str) -> Result<(Vec<u8>, i64), AppError> {
    let mut stmt = conn.prepare(&format!("SELECT * FROM {}", quote_ident(handle)))?;
    let column_names: Vec<String> = stmt.column_names().into_iter().map(str::to_string).collect();
    let mut out = String::new();
    out.push_str(&column_names.iter().map(|c| csv_escape(c)).collect::<Vec<_>>().join(","));
    out.push_str("\r\n");

    let mut rows = stmt.query([])?;
    let mut row_count = 0i64;
    while let Some(row) = rows.next()? {
        let fields: Result<Vec<String>, rusqlite::Error> = (0..column_names.len())
            .map(|i| Ok(csv_escape(&csv_field_string(row.get_ref(i)?))))
            .collect();
        out.push_str(&fields?.join(","));
        out.push_str("\r\n");
        row_count += 1;
    }
    Ok((out.into_bytes(), row_count))
}

async fn build_handle_csv(state: &AppState, tenant: &Tenant, run_id: &str, handle: &str) -> Result<Value, AppError> {
    let path = tables::tenant_db_path(state, tenant.id);
    let handle_for_conn = handle.to_string();
    let now = crate::state::now_unix();
    let (csv_bytes, row_count) = tables::with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
        if !handle_is_live_sync(conn, &handle_for_conn, now)? {
            return Err(handle_not_found(&handle_for_conn));
        }
        build_csv_sync(conn, &handle_for_conn)
    })
    .await?;

    let exports_dir = state.db.data_dir().join("exports");
    tokio::fs::create_dir_all(&exports_dir)
        .await
        .map_err(|e| AppError::Storage(format!("create exports dir: {e}")))?;
    let csv_path = exports_dir.join(format!("{run_id}.csv"));
    tokio::fs::write(&csv_path, &csv_bytes)
        .await
        .map_err(|e| AppError::Storage(format!("write handle csv: {e}")))?;

    let expires_unix = crate::state::now_unix() + crate::export::EXPORT_URL_TTL_SECS;
    let download_url = crate::export::signed_download_url(&state.public_url, &tenant.key_hash, run_id, expires_unix);
    Ok(json!({
        "download_url": download_url,
        "handle": handle,
        "row_count": row_count,
        "size_bytes": csv_bytes.len(),
        "expires_unix": expires_unix,
    }))
}

async fn run_handle_export_job(state: AppState, tenant: Tenant, run_id: String, handle: String) {
    let start = std::time::Instant::now();
    let outcome = build_handle_csv(&state, &tenant, &run_id, &handle).await;
    let finished_unix = crate::state::now_unix();
    let duration_ms = start.elapsed().as_millis() as i64;

    let (status, result_ref, error_class) = match outcome {
        Ok(result_value) => {
            let value_json = serde_json::to_string(&result_value).unwrap_or_else(|_| "null".to_string());
            let bytes = value_json.len() as i64;
            let set_args = json!({"key": crate::runs::part_key(&run_id, 0), "value": result_value});
            match crate::tenant_state::state_set(&state, &tenant, &set_args, None).await {
                Ok(_) => {
                    let result_ref = json!({
                        "parts": 1,
                        "bytes": bytes,
                        "content_type": "application/json",
                    })
                    .to_string();
                    ("done".to_string(), Some(result_ref), None)
                }
                Err(e) => ("error".to_string(), None, Some(e.code().to_string())),
            }
        }
        Err(e) => ("error".to_string(), None, Some(e.code().to_string())),
    };

    let _ = state
        .db
        .finalize_run(run_id, tenant.id, status, result_ref, error_class, None, finished_unix, duration_ms)
        .await;
}

/// `host.table.handle_export(handle)` -- P1 requirement 7/AC8: same
/// runs-ledger/background-job/signed-download shape as `export::export`,
/// writing a CSV of the handle's rows instead of a tar.gz archive.
pub async fn handle_export(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let handle = tables::arg_str(args, "handle")?;

    // Fails fast, before a job is even queued, on a name that isn't a live
    // handle right now -- the same up-front existence check requirement
    // 3/AC3 already makes `host.table.query` take.
    let path = tables::tenant_db_path(state, tenant.id);
    let handle_for_check = handle.clone();
    let now = crate::state::now_unix();
    let exists = tables::with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
        handle_is_live_sync(conn, &handle_for_check, now)
    })
    .await?;
    if !exists {
        return Err(handle_not_found(&handle));
    }

    let plan = tables::plan_of(state, &tenant.plan)?;
    let run_id = crate::state::new_ulid();
    let args_json =
        serde_json::to_string(args).map_err(|e| AppError::Internal(format!("args serialize: {e}")))?;
    let outcome = state
        .db
        .start_export_run(tenant.id, run_id.clone(), HANDLE_EXPORT_TOOL_NAME.to_string(), plan.job_max_s, args_json)
        .await?;
    let run_id = match outcome {
        crate::db::StartExportRun::AlreadyRunning(existing) => {
            return Ok(json!({"run_id": existing, "status": "running"}));
        }
        crate::db::StartExportRun::Started(new_id) => new_id,
    };

    let spawn_state = state.clone();
    let spawn_tenant = tenant.clone();
    let spawn_run_id = run_id.clone();
    let spawn_handle = handle;
    tokio::spawn(async move {
        run_handle_export_job(spawn_state, spawn_tenant, spawn_run_id, spawn_handle).await;
    });
    Ok(json!({"run_id": run_id, "status": "running"}))
}

// ---- background expiry tick (requirement 4/AC4) ---------------------------

/// requirement 4/AC4: reaps every expired handle across every tenant that
/// has ever materialised one. Metadata lives per-tenant (inside each
/// `tables/<tenant_id>.db` file), not in the main database, so this walks
/// `state.db.list_tenants()` rather than a single cross-tenant query --
/// skipping any tenant whose table file doesn't exist yet (never
/// materialised anything) without opening it.
pub async fn tick_once(state: &AppState) -> Result<(), AppError> {
    let tenants = state.db.list_tenants().await?;
    let now = crate::state::now_unix();
    for tenant in tenants {
        let path = tables::tenant_db_path(state, tenant.id);
        if !path.exists() {
            continue;
        }
        let result = tables::with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
            reap_expired_sync(conn, now)
        })
        .await;
        if let Err(e) = result {
            tracing::warn!(error = %e, tenant_id = tenant.id, "result-handle expiry reap failed");
        }
    }
    Ok(())
}

/// Started once alongside this crate's other background tasks (see
/// `main.rs`), same spawn/sleep-loop shape as `tables_model::spawn_tick`.
pub fn spawn_tick(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(TICK_INTERVAL_SECS)).await;
            if let Err(e) = tick_once(&state).await {
                tracing::warn!(error = %e, "result-handle expiry tick failed");
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_handle_id_is_12_lowercase_base32_chars() {
        let id = new_handle_id();
        assert_eq!(id.len(), 12, "id: {id}");
        assert!(
            id.chars().all(|c| c.is_ascii_digit() || c.is_ascii_lowercase()),
            "id must be lowercase base32: {id}"
        );
        assert!(!id.contains('i') && !id.contains('l') && !id.contains('o') && !id.contains('u'), "id: {id}");
    }

    #[test]
    fn extract_handle_names_walks_ctes_joins_and_subqueries() {
        let stmts = tables::parse_single_select(
            "with c as (select * from hdl_abcdefghijkl) \
             select * from c join hdl_mnopqrstuvwx on c.x = hdl_mnopqrstuvwx.x \
             where c.y in (select y from hdl_abcdefghijkl)",
        )
        .expect("parses");
        let names = extract_handle_names(&stmts);
        assert_eq!(names, vec!["hdl_abcdefghijkl".to_string(), "hdl_mnopqrstuvwx".to_string()]);
    }

    #[test]
    fn extract_handle_names_ignores_ordinary_table_names() {
        let stmts = tables::parse_single_select("select * from expenses where category = 'a'").expect("parses");
        assert!(extract_handle_names(&stmts).is_empty());
    }

    #[test]
    fn parse_ttl_s_defaults_clamps_and_rejects() {
        assert_eq!(parse_ttl_s(&json!({})).unwrap(), DEFAULT_TTL_S);
        assert_eq!(parse_ttl_s(&json!({"ttl_s": 1})).unwrap(), 1);
        assert_eq!(parse_ttl_s(&json!({"ttl_s": MAX_TTL_S})).unwrap(), MAX_TTL_S);
        assert!(parse_ttl_s(&json!({"ttl_s": MAX_TTL_S + 1})).is_err());
        assert!(parse_ttl_s(&json!({"ttl_s": 0})).is_err());
        assert!(parse_ttl_s(&json!({"ttl_s": "soon"})).is_err());
    }
}
