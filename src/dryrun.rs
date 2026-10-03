//! PRD-mcphost-dry-run-side-effects P0 requirement 2: the savepoint/
//! rollback machinery every test entry point (`host.tool_test`,
//! `host.tool_run(test: true)`) shares -- a dedicated SQLite connection per
//! store, opened fresh for this one call (never the shared `Db.conn`, and
//! never a tenant's own `tables.rs` connection a concurrent real call might
//! be using), with `SAVEPOINT test_run` opened on first touch so every
//! subsequent write in the same call lands in the same, later-rolled-back
//! transaction however many separate round trips it takes; plus the
//! running list of `dry_run.writes` entries the result envelope reports.
//!
//! `db::Db::with_conn` and `tables::with_tenant_conn` are the only two
//! places that read [`current`] -- every `state.db.*`/`tables::table_*`
//! call already funnels through one of those two, so this is a two-call-site
//! change rather than a signature change to any of the dozens of functions
//! in between. The scope is set once, by [`with_dry_run`], around the whole
//! `Kind::call`/`Kind::tool_run` a test entry point dispatches; every nested
//! `.await` down to the sidecar bridges' own DB calls inherits it (ordinary
//! task-local propagation -- `kinds::python`'s sidecar request/response loop
//! and cold-call dispatch never cross a `tokio::spawn` boundary, so this
//! holds for both the cold and warm-pool call paths).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use serde_json::Value;

use crate::db::DbConfig;
use crate::errors::AppError;

/// One test call's whole dry-run state: the writes reported back to the
/// caller, and the dedicated connections (opened lazily, at most one per
/// store) its bridges write through instead of the real per-call/shared
/// ones.
pub struct DryRunCtx {
    writes: Mutex<Vec<Value>>,
    db_path: PathBuf,
    db_cfg: DbConfig,
    state_conn: Mutex<Option<Arc<Mutex<Connection>>>>,
    table_conns: Mutex<HashMap<PathBuf, Arc<Mutex<Connection>>>>,
}

impl DryRunCtx {
    pub fn new(db_path: PathBuf, db_cfg: DbConfig) -> Arc<Self> {
        Arc::new(Self {
            writes: Mutex::new(Vec::new()),
            db_path,
            db_cfg,
            state_conn: Mutex::new(None),
            table_conns: Mutex::new(HashMap::new()),
        })
    }

    /// Requirement 3: one entry per bridge write this call made, in call
    /// order -- shaped per store by whichever bridge calls this (a table
    /// append records `{store, op, table, rows}`).
    pub fn record_write(&self, write: Value) {
        self.writes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(write);
    }

    pub fn writes(&self) -> Vec<Value> {
        self.writes.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// `db::Db::with_conn`'s dry-run path: a connection to the SAME
    /// `mcphost.db` file `Db` itself uses, opened fresh the first time any
    /// state write in this call needs one, with `SAVEPOINT test_run`
    /// already open -- NOT `Db`'s own shared `conn`, so a long-running test
    /// call never holds the server's single connection for the length of a
    /// sandboxed tool call.
    pub(crate) fn state_conn_sync(&self) -> Result<Arc<Mutex<Connection>>, AppError> {
        let mut guard = self.state_conn.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(conn) = guard.as_ref() {
            return Ok(conn.clone());
        }
        let (conn, _audit) =
            crate::db::open_with_role(&self.db_path, crate::db::ROLE_SERVER, &self.db_cfg)?;
        conn.execute_batch("SAVEPOINT test_run;")?;
        let conn = Arc::new(Mutex::new(conn));
        *guard = Some(conn.clone());
        Ok(conn)
    }

    /// `tables::with_tenant_conn`'s dry-run path: same idea as
    /// [`Self::state_conn_sync`], one dedicated connection per distinct
    /// tenant-table-file `path` this call touches (in practice always
    /// exactly one -- composition stays inside one tenant).
    pub(crate) fn table_conn_sync(&self, path: &Path, cfg: &DbConfig) -> Result<Arc<Mutex<Connection>>, AppError> {
        let mut guard = self.table_conns.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(conn) = guard.get(path) {
            return Ok(conn.clone());
        }
        let conn = crate::tables::open_conn(path, cfg)?;
        conn.execute_batch("SAVEPOINT test_run;")?;
        let conn = Arc::new(Mutex::new(conn));
        guard.insert(path.to_path_buf(), conn.clone());
        Ok(conn)
    }

    /// Rolls back and drops every connection this call opened -- a test run
    /// leaves the tenant's data byte-identical regardless of the call's own
    /// outcome, so the caller runs this unconditionally once `Kind::call`/
    /// `Kind::tool_run` returns (success or error alike).
    fn rollback(&self) {
        if let Some(conn) = self
            .state_conn
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            && let Ok(conn) = conn.lock()
        {
            let _ = conn.execute_batch("ROLLBACK TO test_run; RELEASE test_run;");
        }
        let mut table_conns = self.table_conns.lock().unwrap_or_else(|e| e.into_inner());
        for (_, conn) in table_conns.drain() {
            if let Ok(conn) = conn.lock() {
                let _ = conn.execute_batch("ROLLBACK TO test_run; RELEASE test_run;");
            }
        }
    }
}

tokio::task_local! {
    static CURRENT: Option<Arc<DryRunCtx>>;
}

/// The enclosing call's [`DryRunCtx`], if any -- `None` outside a
/// [`with_dry_run`] scope (every ordinary, non-test call), read by
/// `db::Db::with_conn`/`tables::with_tenant_conn` before they open their
/// own connection.
pub(crate) fn current() -> Option<Arc<DryRunCtx>> {
    CURRENT.try_with(|c| c.clone()).unwrap_or(None)
}

/// Runs `f` with `ctx` visible to every `with_conn`/`with_tenant_conn` call
/// `f` makes, directly or nested arbitrarily deep through ordinary
/// `.await`s, via [`current`].
pub async fn with_dry_run<F: std::future::Future>(ctx: Arc<DryRunCtx>, f: F) -> F::Output {
    CURRENT.scope(Some(ctx), f).await
}

/// Rolls back and releases `ctx`'s connections -- call once, after the test
/// call's `Kind::call`/`Kind::tool_run` has returned.
pub async fn finish(ctx: Arc<DryRunCtx>) {
    let _ = tokio::task::spawn_blocking(move || ctx.rollback()).await;
}
