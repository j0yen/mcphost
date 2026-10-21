//! PRD-mcphost-dry-run-inside-savepoint-for-transactional-verbs R1: the one
//! transaction helper. [`with_tx`] opens `BEGIN IMMEDIATE`/`BEGIN DEFERRED`
//! on an autocommit connection and a nested `SAVEPOINT mcphost_tx` when a
//! transaction is already open (the dry-run connection holds
//! `SAVEPOINT test_run`, see `dryrun.rs`). Both paths issue raw SQL through
//! `execute_batch` so there is exactly one rollback owner: a drop guard.

use crate::errors::AppError;
use rusqlite::Connection;

/// Number of lines in `tests/txn_allowlist.txt` at landing. A later PRD may
/// lower it, never raise it.
pub const TXN_ALLOWLIST_CEILING: usize = 106;

/// Lock behaviour of the outermost transaction (ignored on the nested path).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxBehavior {
    Immediate,
    Deferred,
}

#[cfg(feature = "test-counters")]
pub mod counters {
    //! R6: counts `with_tx` calls that took the nested path, so the dry-run
    //! sweep can report which verbs open transactions. Compiled only under
    //! the `test-counters` feature (integration tests link the lib without
    //! `cfg(test)`).
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NESTED: AtomicUsize = AtomicUsize::new(0);

    pub fn nested_calls() -> usize {
        NESTED.load(Ordering::SeqCst)
    }

    pub(super) fn note_nested() {
        NESTED.fetch_add(1, Ordering::SeqCst);
    }
}

/// Armed from the moment the opening statement succeeds until `COMMIT` or
/// `RELEASE` succeeds. Dropped while armed (panic in `f`, failed
/// `COMMIT`/`RELEASE`), it issues the `Err` path's SQL and ignores failure:
/// a panic in `Drop` during unwinding would abort the process.
struct Guard<'a> {
    conn: &'a Connection,
    undo: &'static str,
    armed: bool,
}

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.conn.execute_batch(self.undo);
        }
    }
}

/// Run `f` inside a transaction (autocommit connection) or a nested
/// savepoint (transaction already open). `Ok` commits/releases; `Err` or a
/// panic rolls back. The nested path never issues `COMMIT` or a bare
/// `ROLLBACK`, which would end the outer `test_run` savepoint.
pub fn with_tx<T>(
    conn: &Connection,
    behavior: TxBehavior,
    f: impl FnOnce(&Connection) -> Result<T, AppError>,
) -> Result<T, AppError> {
    let (open, commit, undo) = if conn.is_autocommit() {
        let open = match behavior {
            TxBehavior::Immediate => "BEGIN IMMEDIATE",
            TxBehavior::Deferred => "BEGIN DEFERRED",
        };
        (open, "COMMIT", "ROLLBACK")
    } else {
        #[cfg(feature = "test-counters")]
        counters::note_nested();
        (
            "SAVEPOINT mcphost_tx",
            "RELEASE mcphost_tx",
            "ROLLBACK TO mcphost_tx; RELEASE mcphost_tx",
        )
    };
    conn.execute_batch(open)?;
    let mut guard = Guard { conn, undo, armed: true };
    match f(conn) {
        Ok(v) => {
            conn.execute_batch(commit)?;
            guard.armed = false;
            Ok(v)
        }
        Err(e) => {
            guard.armed = false;
            let _ = conn.execute_batch(undo);
            Err(e)
        }
    }
}
