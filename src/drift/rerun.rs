//! PRD-mcphost-drift-review requirement 3/4: drains `drift_queue`,
//! replacing ai-stack's `eval.rs` (the external-evaluator stub) with
//! deterministic re-execution of the stored SQL (`table_note`/
//! `table_schema`) or search (`document`), diffs against what was logged,
//! and assembles the review item requirement 5 describes.

use serde_json::{Value, json};

use crate::db::DriftQueueRow;
use crate::errors::AppError;
use crate::state::AppState;
use crate::tables;

use super::{alert, versions};

/// requirement 2: dependent questions are read from the last 30 days of
/// logged activity.
const LOOKBACK_SECS: i64 = 30 * 24 * 60 * 60;

/// requirement 4: at most this many re-runs per change.
const RERUN_CAP: usize = 200;

/// requirement 3: drains every `queued` row, across every tenant, and
/// marks each `done` once processed -- exposed directly (not just via a
/// `spawn_tick`) so a test can drive one deterministic cycle, same
/// convention as `tables_model::tick_once`. One queue row failing to
/// process is logged and still marked `done` (never retried forever) --
/// the same "one broken row must never block every other tenant's cycle"
/// stance `tables_model::tick_once` already takes for a broken table.
pub async fn tick_once(state: &AppState) -> Result<(), AppError> {
    for row in state.db.list_queued_drift().await? {
        if let Err(e) = process_one(state, &row).await {
            tracing::warn!(error = %e, tenant_id = row.tenant_id, target = %row.target, "drift queue row failed to process");
        }
        if let Err(e) = state.db.mark_drift_queue_done(row.id).await {
            tracing::warn!(error = %e, tenant_id = row.tenant_id, "failed to mark drift queue row done");
        }
    }
    Ok(())
}

async fn process_one(state: &AppState, row: &DriftQueueRow) -> Result<(), AppError> {
    let (deltas, changed_count, regressed_count) = match row.kind.as_str() {
        versions::KIND_TABLE_NOTE | versions::KIND_TABLE_SCHEMA => rerun_table(state, row).await?,
        versions::KIND_DOCUMENT => rerun_document(state, row).await?,
        other => {
            tracing::warn!(kind = other, "drift queue row has an unknown kind; skipping");
            return Ok(());
        }
    };

    let item_id = crate::state::new_ulid();
    let old_version = if row.trigger_version > 1 { Some(row.trigger_version - 1) } else { None };

    // requirement 3/AC3: one review per distinct change content -- a
    // duplicate `dedupe_hash` (the same content already reviewed) still
    // reaches `done` above, it just writes no second review row.
    let emitted = state.db.try_emit_review(row.tenant_id, row.dedupe_hash.clone()).await?;
    if emitted {
        state
            .db
            .insert_drift_review(
                item_id.clone(),
                row.tenant_id,
                row.kind.clone(),
                row.target.clone(),
                old_version,
                row.trigger_version,
                row.actor.clone(),
                serde_json::to_string(&deltas).unwrap_or_else(|_| "[]".to_string()),
                changed_count,
                regressed_count,
            )
            .await?;
        alert::raise_if_regressed(state, row.tenant_id, &item_id, &row.target, regressed_count).await?;
    }

    // requirement 4: re-runs count toward the tenant's daily call quota as
    // one call per change, regardless of how many queries/searches this
    // change's own re-run touched (AC9) -- charged once here, whether or
    // not the review above was actually a fresh one (the re-runs
    // themselves still happened either way).
    let _ = state
        .db
        .record_call(
            row.tenant_id,
            "host.drift.tick".to_string(),
            0,
            true,
            None,
            None,
            None,
            "ok",
            "background".to_string(),
            None,
        )
        .await;

    Ok(())
}

/// requirement 2: every name a parsed `sql` string's `FROM`/`JOIN`
/// relations (including inside a `WITH` CTE body or a derived-table
/// subquery) reference -- a deliberately-scoped identifier extractor
/// (same "best-effort over a `sqlparser` AST" stance `chart::base_table_name`
/// already takes), not a general SQL analyzer: it only needs to answer
/// "does this query name `target`", not resolve aliases or schemas.
fn names_in_sql(sql: &str) -> Vec<String> {
    let Ok(statements) = sqlparser::parser::Parser::parse_sql(&sqlparser::dialect::GenericDialect {}, sql) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for stmt in &statements {
        if let sqlparser::ast::Statement::Query(query) = stmt {
            collect_query_names(query, &mut names);
        }
    }
    names
}

fn collect_query_names(query: &sqlparser::ast::Query, out: &mut Vec<String>) {
    if let Some(with) = &query.with {
        for cte in &with.cte_tables {
            collect_query_names(&cte.query, out);
        }
    }
    collect_set_expr_names(&query.body, out);
}

fn collect_set_expr_names(expr: &sqlparser::ast::SetExpr, out: &mut Vec<String>) {
    match expr {
        sqlparser::ast::SetExpr::Select(select) => {
            for twj in &select.from {
                collect_table_factor_names(&twj.relation, out);
                for join in &twj.joins {
                    collect_table_factor_names(&join.relation, out);
                }
            }
        }
        sqlparser::ast::SetExpr::Query(q) => collect_query_names(q, out),
        sqlparser::ast::SetExpr::SetOperation { left, right, .. } => {
            collect_set_expr_names(left, out);
            collect_set_expr_names(right, out);
        }
        _ => {}
    }
}

fn collect_table_factor_names(factor: &sqlparser::ast::TableFactor, out: &mut Vec<String>) {
    match factor {
        sqlparser::ast::TableFactor::Table { name, .. } => out.push(name.to_string()),
        sqlparser::ast::TableFactor::Derived { subquery, .. } => collect_query_names(subquery, out),
        _ => {}
    }
}

/// technical considerations: "`sample_hash` uses the logged column order
/// and the query's own `ORDER BY`" -- a query with no `ORDER BY` gets
/// `unordered: true` on its delta rather than a suppressed `changed` flag.
fn sql_has_order_by(sql: &str) -> bool {
    let Ok(statements) = sqlparser::parser::Parser::parse_sql(&sqlparser::dialect::GenericDialect {}, sql) else {
        return false;
    };
    matches!(statements.as_slice(), [sqlparser::ast::Statement::Query(q)] if q.order_by.is_some())
}

struct LoggedQuery {
    log_id: i64,
    sql: String,
    row_count: Option<i64>,
    error_code: Option<String>,
    sample_hash: Option<String>,
}

/// requirement 2: every `_mcphost_query_log` row from the last 30 days
/// whose SQL names `target`, newest first, capped at [`RERUN_CAP`].
fn dependent_queries_sync(
    conn: &rusqlite::Connection,
    target: &str,
    since: i64,
) -> Result<Vec<LoggedQuery>, AppError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT id, sql, row_count, error_code, sample_hash FROM {} \
         WHERE created_unix >= ?1 ORDER BY id DESC",
        tables::QUERY_LOG_TABLE
    ))?;
    let rows = stmt
        .query_map(rusqlite::params![since], |r| {
            Ok(LoggedQuery {
                log_id: r.get(0)?,
                sql: r.get(1)?,
                row_count: r.get(2)?,
                error_code: r.get(3)?,
                sample_hash: r.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows
        .into_iter()
        .filter(|q| names_in_sql(&q.sql).iter().any(|n| n.eq_ignore_ascii_case(target)))
        .take(RERUN_CAP)
        .collect())
}

/// requirement 4: re-executes one logged query's `sql` against the
/// tenant's current table store -- `(row_count, sample_hash, error_code)`,
/// the exact shape a `before` or `after` side of a [`QueryDelta`] needs.
fn execute_for_delta(conn: &rusqlite::Connection, sql: &str) -> (Option<i64>, Option<String>, Option<String>) {
    match tables::validate_query_structure(sql).and_then(|_query| tables::run_query_sync(conn, sql)) {
        Ok(rows) => (Some(rows.len() as i64), Some(tables::sample_hash_of(&rows)), None),
        Err(e) => (None, None, Some(e.code().to_string())),
    }
}

/// requirement 2/4/5: re-runs every logged query over `row.target`, builds
/// `QueryDelta` entries, and returns `(deltas, changed_count,
/// regressed_count)`.
async fn rerun_table(state: &AppState, row: &DriftQueueRow) -> Result<(Vec<Value>, i64, i64), AppError> {
    let since = crate::state::now_unix() - LOOKBACK_SECS;
    let path = tables::tenant_db_path(state, row.tenant_id);
    let target = row.target.clone();
    let logged = tables::with_tenant_conn(path.clone(), state.db.cfg(), state.db.counters_handle(), move |conn| {
        dependent_queries_sync(conn, &target, since)
    })
    .await?;

    let deltas = tables::with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
        let mut deltas = Vec::with_capacity(logged.len());
        for q in &logged {
            let (after_row_count, after_sample_hash, after_error_code) = execute_for_delta(conn, &q.sql);
            let unordered = !sql_has_order_by(&q.sql);
            let changed = after_row_count != q.row_count
                || after_sample_hash != q.sample_hash
                || after_error_code != q.error_code;
            let was_ok_with_rows = q.error_code.is_none() && q.row_count.unwrap_or(0) > 0;
            let now_errors = after_error_code.is_some();
            let now_zero_rows = after_error_code.is_none() && after_row_count == Some(0);
            let regressed = was_ok_with_rows && (now_errors || now_zero_rows);
            deltas.push(json!({
                "kind": "query",
                "log_id": q.log_id,
                "sql": q.sql,
                "before": {"row_count": q.row_count, "sample_hash": q.sample_hash, "error_code": q.error_code},
                "after": {"row_count": after_row_count, "sample_hash": after_sample_hash, "error_code": after_error_code},
                "changed": changed,
                "unordered": unordered,
                "regressed": regressed,
            }));
        }
        Ok::<_, AppError>(deltas)
    })
    .await?;

    let changed_count = deltas.iter().filter(|d| d["changed"] == json!(true)).count() as i64;
    let regressed_count = deltas.iter().filter(|d| d["regressed"] == json!(true)).count() as i64;
    Ok((deltas, changed_count, regressed_count))
}

struct LoggedSearch {
    log_id: i64,
    query: String,
    mode: String,
    top_ids: Vec<Value>,
}

/// requirement 2: every `_docs_search_log` row from the last 30 days whose
/// `top_ids` named `target` (by document name), newest first, capped at
/// [`RERUN_CAP`].
fn dependent_searches_sync(
    conn: &rusqlite::Connection,
    target: &str,
    since: i64,
) -> Result<Vec<LoggedSearch>, AppError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT id, query, mode, top_ids_json FROM {} WHERE created_unix >= ?1 ORDER BY id DESC",
        tables::DOCS_SEARCH_LOG_TABLE
    ))?;
    let rows = stmt
        .query_map(rusqlite::params![since], |r| {
            let top_ids_json: String = r.get(3)?;
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, top_ids_json))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut out = Vec::new();
    for (log_id, query, mode, top_ids_json) in rows {
        let top_ids: Vec<Value> = serde_json::from_str(&top_ids_json).unwrap_or_default();
        let names_hit = top_ids.iter().any(|v| v["name"].as_str() == Some(target));
        if names_hit {
            out.push(LoggedSearch { log_id, query, mode, top_ids });
        }
        if out.len() >= RERUN_CAP {
            break;
        }
    }
    Ok(out)
}

/// requirement 2/4/5/6 (AC6): re-runs every logged search whose top hits
/// named `row.target`, builds `SearchDelta` entries (top-5 overlap by
/// `(name, chunk_no)` identity), and returns `(deltas, changed_count,
/// regressed_count)` -- `regressed` when overlap falls under 3.
async fn rerun_document(state: &AppState, row: &DriftQueueRow) -> Result<(Vec<Value>, i64, i64), AppError> {
    let since = crate::state::now_unix() - LOOKBACK_SECS;
    let path = tables::tenant_db_path(state, row.tenant_id);
    let target = row.target.clone();
    let logged = tables::with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
        dependent_searches_sync(conn, &target, since)
    })
    .await?;

    let mut deltas = Vec::with_capacity(logged.len());
    for s in &logged {
        let after_results = crate::docs::rerun_search(state, row.tenant_id, &s.query, &s.mode).await?;
        let after_top5: Vec<Value> = after_results
            .iter()
            .take(5)
            .map(|r| json!({"name": r["name"], "chunk_no": r["chunk_no"]}))
            .collect();
        let before_top5: Vec<Value> = s.top_ids.iter().take(5).cloned().collect();
        let overlap = before_top5.iter().filter(|b| after_top5.contains(b)).count() as i64;
        let regressed = overlap < 3;
        let changed = before_top5 != after_top5;
        deltas.push(json!({
            "kind": "search",
            "log_id": s.log_id,
            "query": s.query,
            "before_top5": before_top5,
            "after_top5": after_top5,
            "overlap": overlap,
            "changed": changed,
            "regressed": regressed,
        }));
    }

    let changed_count = deltas.iter().filter(|d| d["changed"] == json!(true)).count() as i64;
    let regressed_count = deltas.iter().filter(|d| d["regressed"] == json!(true)).count() as i64;
    Ok((deltas, changed_count, regressed_count))
}
