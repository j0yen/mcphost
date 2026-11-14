//! `host.table.exec`: one INSERT, UPDATE or DELETE over a declared table, and the
//! statement grammar every table-SQL refusal carries (`data.allowed`).
//!
//! PRD-mcphost-table-sql-exec-and-grammar. The grammar has one home: [`FORMS`]
//! lists every accepted statement shape as text, [`EXEC_ALLOWED`] and
//! [`QUERY_ALLOWED`] list the statement kinds each verb takes, and the validators
//! below read only those lists -- so the refusal text and the validator cannot
//! drift (`tests/tsql_ac04_*` parses each form back to its kind and compares).
//!
//! The validator is the security gate (R12): it parses the statement, refuses
//! every kind but the allowed ones, then walks the whole AST and refuses
//! anything that could reach past the tenant's own tables -- `ATTACH`,
//! `PRAGMA`, triggers and other DDL, file or extension functions, table-valued
//! functions outside a closed list, and a nested statement or subquery that is
//! not a plain `SELECT`. WHERE-less UPDATE and DELETE are refused from the AST
//! (`selection.is_none()`), never by matching text.

use std::ops::ControlFlow;

use rusqlite::Connection;
use serde_json::{Value, json};
use sqlparser::ast::{
    Delete, Expr, FromTable, Insert, ObjectName, OnInsert, Query, SetExpr, SqliteOnConflict, Statement, TableFactor,
    TableObject, Update, Visit, Visitor,
};

use crate::db::Tenant;
use crate::errors::AppError;
use crate::state::AppState;
use crate::tables;
use crate::txn::{TxBehavior, with_tx};

/// The statement kinds the table-SQL surface speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StatementKind {
    Select,
    Insert,
    Update,
    Delete,
}

impl StatementKind {
    /// The lowercase name `host.table.query_log` reports as `statement_kind`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Select => "select",
            Self::Insert => "insert",
            Self::Update => "update",
            Self::Delete => "delete",
        }
    }

    /// The statement kind a grammar form (or any SQL text) opens with, read from
    /// its first keyword: `SELECT`/`WITH` are a select, the rest name themselves.
    pub fn from_leading_keyword(text: &str) -> Option<Self> {
        let word = text.trim_start().split(|c: char| !c.is_ascii_alphabetic()).next()?;
        match word.to_ascii_uppercase().as_str() {
            "SELECT" | "WITH" => Some(Self::Select),
            "INSERT" => Some(Self::Insert),
            "UPDATE" => Some(Self::Update),
            "DELETE" => Some(Self::Delete),
            _ => None,
        }
    }
}

/// R1: the kinds `host.table.exec` accepts.
pub const EXEC_ALLOWED: &[StatementKind] = &[StatementKind::Insert, StatementKind::Update, StatementKind::Delete];

/// The kinds `host.table.query` accepts.
pub const QUERY_ALLOWED: &[StatementKind] = &[StatementKind::Select];

/// One accepted statement shape: the text a refusal's `data.allowed` lists, and an
/// `example` over a table `t(id, a)` that the validator accepts as `kind`.
#[derive(Clone, Copy, Debug)]
pub struct StatementForm {
    pub kind: StatementKind,
    pub form: &'static str,
    pub example: &'static str,
}

const fn form(kind: StatementKind, form: &'static str, example: &'static str) -> StatementForm {
    StatementForm { kind, form, example }
}

/// Every accepted statement shape. A kind in [`EXEC_ALLOWED`] or [`QUERY_ALLOWED`]
/// with no row here, or a row whose kind is in neither, fails `tsql_ac04`; so does
/// an `example` the validator refuses.
pub const FORMS: &[StatementForm] = &[
    form(StatementKind::Select, "SELECT … FROM <table> …", "SELECT * FROM t WHERE id = 1"),
    form(
        StatementKind::Select,
        "WITH <name> AS (SELECT …) SELECT …",
        "WITH x AS (SELECT id FROM t) SELECT * FROM x",
    ),
    form(StatementKind::Insert, "INSERT INTO <table> (<columns>) VALUES (…)", "INSERT INTO t (id, a) VALUES (1, 1)"),
    form(
        StatementKind::Insert,
        "INSERT OR REPLACE INTO <table> (<columns>) VALUES (…)",
        "INSERT OR REPLACE INTO t (id, a) VALUES (1, 1)",
    ),
    form(
        StatementKind::Insert,
        "INSERT INTO <table> (<columns>) VALUES (…) ON CONFLICT(<column>) DO UPDATE SET …",
        "INSERT INTO t (id, a) VALUES (1, 1) ON CONFLICT(id) DO UPDATE SET a = excluded.a",
    ),
    form(StatementKind::Update, "UPDATE <table> SET <column> = … WHERE …", "UPDATE t SET a = 1 WHERE id = 1"),
    form(StatementKind::Delete, "DELETE FROM <table> WHERE …", "DELETE FROM t WHERE id = 1"),
];

/// R2: the one function that renders the grammar -- the refusal text, the tool
/// descriptions and `docs/tools.md` all read it.
pub fn allowed_statement_forms() -> Vec<String> {
    FORMS.iter().map(|f| f.form.to_string()).collect()
}

/// The kinds the grammar forms spell, sorted and de-duplicated -- what `tsql_ac04`
/// compares to the validator's accepted set.
pub fn allowed_kinds_from_forms() -> Vec<StatementKind> {
    let mut kinds: Vec<StatementKind> = FORMS.iter().map(|f| f.kind).collect();
    kinds.sort();
    kinds.dedup();
    kinds
}

/// The kind the validators accept `sql` as, across `host.table.query` (a select) and
/// `host.table.exec`; `None` when both refuse it.
pub fn accepted_kind(sql: &str) -> Option<StatementKind> {
    if let Ok(statement) = validate_exec_structure(sql) {
        return Some(statement.kind);
    }
    crate::tables::validate_query_structure(sql).ok().map(|_| StatementKind::Select)
}

/// The table functions a statement may read from (`json_each(...)` and friends).
/// Every other table-valued function -- `pragma_*`, `readfile`, `fsdir`, ... -- is
/// refused.
pub const ALLOWED_TABLE_FUNCTIONS: &[&str] = &["json_each", "json_tree", "generate_series"];

/// R12: scalar functions that read or write the host filesystem, or load code.
pub const FORBIDDEN_FUNCTIONS: &[&str] =
    &["load_extension", "readfile", "writefile", "edit", "fts3_tokenizer", "sqlite_attach", "zipfile"];

/// Table-name prefixes the host reserves for its own bookkeeping tables.
const RESERVED_PREFIXES: &[&str] = &["_mcphost_", "_docs_", "sqlite_"];

/// Which verb a refusal belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verb {
    Query,
    Exec,
}

/// `(line, column)` of a sqlparser failure, read from the `at Line: n, Column: m`
/// suffix its messages carry; `None` when the message has none.
pub fn parser_location(error: &sqlparser::parser::ParserError) -> Option<(u64, u64)> {
    use sqlparser::parser::ParserError;
    let message = match error {
        ParserError::TokenizerError(m) | ParserError::ParserError(m) => m,
        ParserError::RecursionLimitExceeded => return None,
    };
    let tail = &message[message.rfind("Line: ")? + "Line: ".len()..];
    let (line, rest) = tail.split_once(", Column: ")?;
    let column: String = rest.chars().take_while(char::is_ascii_digit).collect();
    Some((line.trim().parse().ok()?, column.parse().ok()?))
}

/// R2: a `table_query_rejected` / `table_exec_rejected` refusal with
/// `data: {reason, line, column, allowed}`.
pub fn rejected(verb: Verb, reason: impl Into<String>, location: Option<(u64, u64)>) -> AppError {
    let reason = reason.into();
    let (code, noun) = match verb {
        Verb::Query => (crate::tables::TABLE_QUERY_REJECTED, "query"),
        Verb::Exec => (crate::tables::TABLE_EXEC_REJECTED, "exec"),
    };
    AppError::Structured {
        code,
        message: format!("{noun} rejected: {reason}"),
        data: json!({
            "reason": reason,
            "line": location.map(|(line, _)| line),
            "column": location.map(|(_, column)| column),
            "allowed": allowed_statement_forms(),
        }),
    }
}

/// [`rejected`] plus extra `data` keys (`host.table.find`'s `allowed_operators` and
/// `columns`).
pub fn rejected_with(verb: Verb, reason: impl Into<String>, extra: Value) -> AppError {
    match (rejected(verb, reason, None), extra) {
        (AppError::Structured { code, message, mut data }, Value::Object(extra)) => {
            if let Some(data) = data.as_object_mut() {
                data.extend(extra);
            }
            AppError::Structured { code, message, data }
        }
        (error, _) => error,
    }
}

fn exec_rejected(reason: impl Into<String>) -> AppError {
    rejected(Verb::Exec, reason, None)
}

/// A validated `host.table.exec` statement.
#[derive(Debug)]
pub struct ExecStatement {
    pub kind: StatementKind,
    /// The one table the statement writes.
    pub table: String,
}

/// R1/R12: parses `sql` and returns what it writes, or refuses it
/// `table_exec_rejected`.
pub fn validate_exec_structure(sql: &str) -> Result<ExecStatement, AppError> {
    let mut statements = sqlparser::parser::Parser::parse_sql(&sqlparser::dialect::GenericDialect {}, sql).map_err(|e| {
        // A PRAGMA or other keyword-led statement the generic dialect cannot parse is
        // still refused for what it is, not for where parsing stopped; a misspelling
        // (not a keyword) keeps the parser's position.
        let first = leading_words(sql);
        let word = first.split(' ').next().unwrap_or_default();
        if StatementKind::from_leading_keyword(sql).is_none()
            && sqlparser::keywords::ALL_KEYWORDS.binary_search_by(|keyword| (*keyword).cmp(word)).is_ok()
        {
            return exec_rejected(format!("{first} is not allowed; host.table.exec takes one INSERT, UPDATE or DELETE"));
        }
        rejected(Verb::Exec, format!("sql parse error: {e}"), parser_location(&e))
    })?;
    let statement = match statements.len() {
        0 => return Err(exec_rejected("empty statement")),
        1 => statements.remove(0),
        _ => return Err(exec_rejected("multiple statements are not allowed; submit exactly one INSERT, UPDATE or DELETE")),
    };
    let (kind, table) = match &statement {
        Statement::Insert(insert) => (StatementKind::Insert, check_insert(insert)?),
        Statement::Update(update) => (StatementKind::Update, check_update(update)?),
        Statement::Delete(delete) => (StatementKind::Delete, check_delete(delete)?),
        Statement::Query(_) => {
            return Err(exec_rejected("a SELECT belongs to host.table.query; host.table.exec takes one INSERT, UPDATE or DELETE"));
        }
        _ => return Err(exec_rejected(format!("{} is not allowed; host.table.exec takes one INSERT, UPDATE or DELETE", leading_words(sql)))),
    };
    if !EXEC_ALLOWED.contains(&kind) {
        return Err(exec_rejected(format!("{} is not allowed", kind.as_str())));
    }
    walk(&statement)?;
    if table.starts_with(crate::tables::HANDLE_PREFIX) {
        return Err(exec_rejected(format!("'{table}' is a query-result handle, which is read-only")));
    }
    Ok(ExecStatement { kind, table })
}

/// The statement's opening words for a refusal: `CREATE TRIGGER`, `PRAGMA`, `ATTACH`.
fn leading_words(sql: &str) -> String {
    let mut words = sql.split_whitespace().map(str::to_ascii_uppercase);
    let first = words.next().unwrap_or_default();
    match (first.as_str(), words.next()) {
        ("CREATE" | "DROP" | "ALTER", Some(second)) => format!("{first} {second}"),
        _ => first,
    }
}

/// The single table name an `ObjectName` spells, or a refusal.
fn plain_table_name(name: &ObjectName) -> Result<String, AppError> {
    match name.0.as_slice() {
        [part] => part
            .as_ident()
            .map(|ident| ident.value.clone())
            .ok_or_else(|| exec_rejected("the target must be a plain table name")),
        _ => Err(exec_rejected("the target must be one declared table, with no schema or database prefix")),
    }
}

fn check_insert(insert: &Insert) -> Result<String, AppError> {
    let TableObject::TableName(name) = &insert.table else {
        return Err(exec_rejected("the INSERT target must be a plain table name"));
    };
    if insert.returning.is_some() {
        return Err(exec_rejected("RETURNING is not supported; read the row back with host.table.query"));
    }
    if insert.or == Some(SqliteOnConflict::Rollback) {
        return Err(exec_rejected("OR ROLLBACK is not allowed"));
    }
    if insert.replace_into || insert.ignore || insert.overwrite || insert.partitioned.is_some() {
        return Err(exec_rejected("use INSERT [OR REPLACE] INTO <table> (<columns>) VALUES (…)"));
    }
    if let Some(on) = &insert.on
        && !matches!(on, OnInsert::OnConflict(_))
    {
        return Err(exec_rejected("only ON CONFLICT(<column>) DO UPDATE is accepted as an upsert"));
    }
    plain_table_name(name)
}

fn check_update(update: &Update) -> Result<String, AppError> {
    if update.selection.is_none() {
        return Err(exec_rejected("UPDATE needs a WHERE clause: UPDATE <table> SET <column> = … WHERE …"));
    }
    if update.returning.is_some() {
        return Err(exec_rejected("RETURNING is not supported; read the row back with host.table.query"));
    }
    if update.from.is_some() || !update.table.joins.is_empty() || update.limit.is_some() || !update.order_by.is_empty() {
        return Err(exec_rejected("UPDATE takes one table with SET … WHERE …; no FROM, JOIN, ORDER BY or LIMIT"));
    }
    if update.or == Some(SqliteOnConflict::Rollback) {
        return Err(exec_rejected("OR ROLLBACK is not allowed"));
    }
    match &update.table.relation {
        TableFactor::Table { name, args: None, .. } => plain_table_name(name),
        _ => Err(exec_rejected("the UPDATE target must be a plain table name")),
    }
}

fn check_delete(delete: &Delete) -> Result<String, AppError> {
    if delete.selection.is_none() {
        return Err(exec_rejected("DELETE needs a WHERE clause: DELETE FROM <table> WHERE …"));
    }
    if delete.returning.is_some() {
        return Err(exec_rejected("RETURNING is not supported; read the row back with host.table.query"));
    }
    if !delete.tables.is_empty() || delete.using.is_some() || delete.limit.is_some() || !delete.order_by.is_empty() {
        return Err(exec_rejected("DELETE takes DELETE FROM <table> WHERE …; no USING, ORDER BY or LIMIT"));
    }
    let FromTable::WithFromKeyword(from) = &delete.from else {
        return Err(exec_rejected("write DELETE FROM <table> WHERE …"));
    };
    match from.as_slice() {
        [one] if one.joins.is_empty() => match &one.relation {
            TableFactor::Table { name, args: None, .. } => plain_table_name(name),
            _ => Err(exec_rejected("the DELETE target must be a plain table name")),
        },
        _ => Err(exec_rejected("DELETE takes exactly one table")),
    }
}

/// R12: walks the statement's whole AST and refuses what could reach past the
/// tenant's own tables.
fn walk(statement: &Statement) -> Result<(), AppError> {
    let mut gate = Gate { statements: 0 };
    match statement.visit(&mut gate) {
        ControlFlow::Break(reason) => Err(exec_rejected(reason)),
        ControlFlow::Continue(()) if gate.statements == 1 => Ok(()),
        ControlFlow::Continue(()) => Err(exec_rejected("a nested statement is not allowed")),
    }
}

/// The R12 visitor. `Break` carries the refusal reason.
struct Gate {
    statements: usize,
}

impl Gate {
    fn plain_query(query: &Query) -> ControlFlow<String> {
        match query.body.as_ref() {
            SetExpr::Insert(_) | SetExpr::Update(_) | SetExpr::Delete(_) | SetExpr::Merge(_) => {
                ControlFlow::Break("a subquery must be a plain SELECT".to_string())
            }
            SetExpr::Table(_) => ControlFlow::Break("a subquery must be a plain SELECT, not TABLE".to_string()),
            _ => ControlFlow::Continue(()),
        }
    }
}

impl Visitor for Gate {
    type Break = String;

    fn pre_visit_statement(&mut self, _statement: &Statement) -> ControlFlow<String> {
        self.statements += 1;
        ControlFlow::Continue(())
    }

    fn pre_visit_query(&mut self, query: &Query) -> ControlFlow<String> {
        Self::plain_query(query)
    }

    fn pre_visit_relation(&mut self, relation: &ObjectName) -> ControlFlow<String> {
        let name = relation.to_string().trim_matches('"').to_ascii_lowercase();
        if relation.0.len() != 1 {
            return ControlFlow::Break(format!("'{relation}' is not a declared table: no schema or database prefix is allowed"));
        }
        if RESERVED_PREFIXES.iter().any(|prefix| name.starts_with(prefix)) {
            return ControlFlow::Break(format!("'{relation}' is a reserved table"));
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_table_factor(&mut self, factor: &TableFactor) -> ControlFlow<String> {
        match factor {
            TableFactor::Table { name, args: Some(_), .. } => {
                let function = name.to_string().to_ascii_lowercase();
                if ALLOWED_TABLE_FUNCTIONS.contains(&function.as_str()) {
                    ControlFlow::Continue(())
                } else {
                    ControlFlow::Break(format!("the table function '{name}' is not allowed"))
                }
            }
            TableFactor::Table { .. } | TableFactor::Derived { .. } | TableFactor::NestedJoin { .. } => {
                ControlFlow::Continue(())
            }
            _ => ControlFlow::Break("only tables and subqueries may appear in FROM".to_string()),
        }
    }

    fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<String> {
        match expr {
            Expr::Function(function) => {
                let name = function.name.to_string().to_ascii_lowercase();
                if FORBIDDEN_FUNCTIONS.contains(&name.as_str()) {
                    return ControlFlow::Break(format!("the function {name}() is not allowed"));
                }
                ControlFlow::Continue(())
            }
            Expr::Subquery(query) | Expr::Exists { subquery: query, .. } => Self::plain_query(query),
            Expr::InSubquery { subquery, .. } => Self::plain_query(subquery),
            _ => ControlFlow::Continue(()),
        }
    }
}

/// Restores `PRAGMA query_only` to what it was when [`QueryOnlyGuard::off`] ran, on
/// every exit path (R11). The read path leaves it `ON`; an exec needs it `OFF`.
pub struct QueryOnlyGuard<'a> {
    conn: &'a Connection,
    prior_on: bool,
}

impl<'a> QueryOnlyGuard<'a> {
    pub fn off(conn: &'a Connection) -> Result<Self, AppError> {
        let prior_on = conn.pragma_query_value(None, "query_only", |row| row.get::<_, i64>(0))? != 0;
        conn.pragma_update(None, "query_only", "OFF")?;
        Ok(Self { conn, prior_on })
    }
}

impl Drop for QueryOnlyGuard<'_> {
    fn drop(&mut self) {
        let state = if self.prior_on { "ON" } else { "OFF" };
        let _ = self.conn.pragma_update(None, "query_only", state);
    }
}

/// The wire shape of a successful exec.
pub fn exec_response(table: &str, changed: usize) -> Value {
    json!({"changed": changed, "table": table})
}

/// R1/R5/R9/R11: `host.table.exec {sql}` -- one INSERT, UPDATE or DELETE on a
/// declared table, run in one immediate transaction under [`tables::QUERY_TIME_CAP`].
/// R7: every call, refused or run, leaves one row in the tenant's query log.
pub async fn table_exec(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let sql = tables::arg_str(args, "sql")?;
    let path = tables::tenant_db_path(state, tenant.id);
    let statement = match validate_exec_structure(&sql) {
        Ok(statement) => statement,
        Err(refusal) => {
            log_refusal(state, path, &sql, &refusal).await;
            return Err(refusal);
        }
    };
    let ExecStatement { kind, table } = statement;

    let plan = tables::plan_of(state, &tenant.plan)?;
    let run = ExecRun {
        sql,
        kind,
        table: table.clone(),
        rows_max: plan.table_rows_max,
        bytes_max: plan.table_bytes_max,
        plan_name: tenant.plan.clone(),
        catalog: state.plans.clone(),
        billing_mode: state.billing_config.billing_mode(),
    };
    let changed = tables::with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
        let started = std::time::Instant::now();
        let _restore = QueryOnlyGuard::off(conn)?;
        let result = run.execute(conn);
        tables::log_query(
            conn,
            &run.sql,
            tables::QueryLogOutcome {
                duration_ms: started.elapsed().as_millis() as i64,
                row_count: None,
                sample_hash: None,
                error: result.as_ref().err(),
                diagnosis: None,
                hint: None,
                result_bytes: None,
                est_tokens: None,
                exec: Some(tables::ExecLog {
                    statement_kind: Some(run.kind.as_str()),
                    changed: result.as_ref().ok().map(|changed| *changed as i64),
                }),
            },
        );
        result
    })
    .await?;

    crate::dryrun::note_would("rows", json!({"table": table, "action": kind.as_str(), "changed": changed}));
    if changed > 0 {
        if let Err(e) = state.db.mark_table_model_stale(tenant.id, table.clone()).await {
            tracing::warn!(error = %e, table = %table, "failed to mark table model stale after exec");
        }
        if let Err(e) = state.db.mark_table_graph_stale(tenant.id).await {
            tracing::warn!(error = %e, table = %table, "failed to mark table graph stale after exec");
        }
        tables::bump_change_counter(state, tenant.id, &table, kind != StatementKind::Insert).await;
    }
    Ok(exec_response(&table, changed))
}

/// Logs the one row a structurally refused exec leaves: its kind when its text opens
/// with one, no `changed`, the refusal's code.
async fn log_refusal(state: &AppState, path: std::path::PathBuf, sql: &str, refusal: &AppError) {
    let (sql, refusal) = (sql.to_string(), refusal.clone());
    let logged = tables::with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
        tables::log_query(
            conn,
            &sql,
            tables::QueryLogOutcome {
                duration_ms: 0,
                row_count: None,
                sample_hash: None,
                error: Some(&refusal),
                diagnosis: None,
                hint: None,
                result_bytes: None,
                est_tokens: None,
                exec: Some(tables::ExecLog {
                    statement_kind: StatementKind::from_leading_keyword(&sql).map(StatementKind::as_str),
                    changed: None,
                }),
            },
        );
        Ok(())
    })
    .await;
    if let Err(e) = logged {
        tracing::warn!(error = %e, "failed to log a refused host.table.exec attempt");
    }
}

/// One validated exec with the plan ceilings it runs under.
struct ExecRun {
    sql: String,
    kind: StatementKind,
    table: String,
    rows_max: i64,
    bytes_max: i64,
    plan_name: String,
    catalog: crate::plans::PlanCatalog,
    billing_mode: &'static str,
}

impl ExecRun {
    /// R5: checks the ceilings around one statement inside one immediate transaction; an
    /// overrun rolls the statement back.
    fn execute(&self, conn: &Connection) -> Result<usize, AppError> {
        tables::load_schema_sync(conn, &self.table)?;
        with_tx(conn, TxBehavior::Immediate, |conn| {
            let existing_rows = tables::row_count_sync(conn, &self.table)?;
            if self.kind != StatementKind::Delete {
                let file_bytes = conn.path().and_then(|p| std::fs::metadata(p).ok()).map_or(0, |m| m.len() as i64);
                if file_bytes + self.sql.len() as i64 > self.bytes_max {
                    return Err(self.quota_exceeded("table_bytes_max", self.bytes_max, file_bytes));
                }
            }
            let changed = execute_capped(conn, &self.sql)?;
            if self.kind == StatementKind::Insert {
                let rows_after = tables::row_count_sync(conn, &self.table)?;
                if rows_after > self.rows_max && rows_after > existing_rows {
                    return Err(self.quota_exceeded("table_rows_max", self.rows_max, existing_rows));
                }
            }
            Ok(changed)
        })
    }

    /// The refusal `table_append` gives for the same ceiling: same code, same `data` keys.
    fn quota_exceeded(&self, limit: &'static str, value: i64, used: i64) -> AppError {
        crate::billing::quota_exceeded_for(&self.catalog, self.billing_mode, &self.plan_name, limit, value, used, None)
    }
}

/// Runs one validated write statement, interrupting it past
/// [`tables::QUERY_TIME_CAP`]; a statement that would return rows is refused.
fn execute_capped(conn: &Connection, sql: &str) -> Result<usize, AppError> {
    let interrupt = conn.get_interrupt_handle();
    std::thread::spawn(move || {
        std::thread::sleep(tables::QUERY_TIME_CAP);
        interrupt.interrupt();
    });
    let mut stmt = conn.prepare(sql).map_err(sqlite_refusal)?;
    if stmt.readonly() {
        return Err(exec_rejected("a SELECT belongs to host.table.query"));
    }
    stmt.execute([]).map_err(sqlite_refusal)
}

/// SQLite's own refusal of a statement that parsed: a missing column, a type or
/// constraint failure, an interrupt past the time cap.
fn sqlite_refusal(error: rusqlite::Error) -> AppError {
    match &error {
        rusqlite::Error::SqliteFailure(failure, _) if failure.code == rusqlite::ErrorCode::OperationInterrupted => {
            tables::bound_exceeded("time_cap_s", tables::QUERY_TIME_CAP.as_secs() as i64)
        }
        rusqlite::Error::SqliteFailure(failure, Some(message))
            if failure.code == rusqlite::ErrorCode::Unknown || failure.extended_code == rusqlite::ffi::SQLITE_ERROR =>
        {
            exec_rejected(format!("sqlite: {message}"))
        }
        _ => AppError::from(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query_only(conn: &Connection) -> i64 {
        conn.pragma_query_value(None, "query_only", |row| row.get(0)).unwrap()
    }

    #[test]
    fn parser_location_reads_the_line_and_column_sqlparser_embeds() {
        let parse = |sql: &str| {
            sqlparser::parser::Parser::parse_sql(&sqlparser::dialect::GenericDialect {}, sql).unwrap_err()
        };
        assert_eq!(parser_location(&parse("SELEC * FROM t")), Some((1, 1)));
        assert_eq!(parser_location(&parse("SELECT *\nFROM t WHERE )")).map(|(line, _)| line), Some(2));
        assert_eq!(parser_location(&sqlparser::parser::ParserError::ParserError("no position".to_string())), None);
        assert_eq!(parser_location(&sqlparser::parser::ParserError::RecursionLimitExceeded), None);
    }

    #[test]
    fn the_guard_restores_the_prior_query_only_state_on_every_exit() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "query_only", "ON").unwrap();
        {
            let _guard = QueryOnlyGuard::off(&conn).unwrap();
            assert_eq!(query_only(&conn), 0, "an exec runs with query_only OFF");
        }
        assert_eq!(query_only(&conn), 1, "a read-only connection goes back to read-only");

        let result: Result<(), AppError> = (|| {
            let _guard = QueryOnlyGuard::off(&conn)?;
            Err(exec_rejected("early return"))
        })();
        assert!(result.is_err());
        assert_eq!(query_only(&conn), 1, "an error exit restores it too");

        conn.pragma_update(None, "query_only", "OFF").unwrap();
        drop(QueryOnlyGuard::off(&conn).unwrap());
        assert_eq!(query_only(&conn), 0, "a writable connection stays writable");
    }
}
