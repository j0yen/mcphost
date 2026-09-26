//! Shared tracing-capture helper for the SQLite busy-timeout-audit AC
//! files that read startup pragma-audit log lines (AC1, AC2) -- not a
//! crate module, pulled in per-file with `#[path = "support/busyaudit.rs"]
//! mod busyaudit;`, same convention as `tests/support/host.rs`.

use std::sync::{Arc, Mutex, Once};

#[derive(Clone, Default)]
pub struct CapturedLog(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for CapturedLog {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedLog {
    type Writer = CapturedLog;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// `db.rs`'s two `db_audit` `tracing::info!` call sites (one per role) are
/// hit by every unrelated `Db::open`/`TestServer::start` in this whole
/// suite binary, almost all of them with no subscriber installed. Whichever
/// thread hits either call site *first ever in the process* decides that
/// call site's cached "interest" (`tracing`'s per-callsite interest cache is
/// process-global, memoized on first hit -- see `capture_tracing` below).
/// If that first hit lands on a no-subscriber thread, `tracing` caches
/// "never interested" for it, and nothing but an explicit
/// `rebuild_interest_cache()` call recomputes it.
///
/// `capture_tracing` already calls `rebuild_interest_cache()` before
/// running `f`, but that alone only closes the race for a call site that
/// was *already* registered by the time it runs -- rebuilding is a no-op
/// for a call site nobody has hit yet. In a large suite binary, that gap is
/// real: an unrelated `Db::open` on another thread can register one of
/// these two call sites for the first time in the narrow window between
/// this test's own `rebuild_interest_cache()` and its own two audit log
/// lines (the two `open_with_role` calls in between do real disk I/O), and
/// if it does, that thread's own no-subscriber dispatch is what gets
/// cached -- permanently, since nothing rebuilds again afterward.
///
/// Forcing one throwaway `Db::open_with_cfg` here, before the scoped
/// subscriber (and its `rebuild_interest_cache()`) are ever installed,
/// guarantees both call sites are *already* registered by the time that
/// rebuild runs -- whatever interest this warm-up call itself raced to
/// cache doesn't matter, only that the call sites now exist for the
/// rebuild to recompute against our subscriber. Once registered, nothing
/// else in this binary ever calls `rebuild_interest_cache()` again, so the
/// "always interested" it sets is permanent for the rest of the process.
fn warm_up_db_audit_callsites() {
    static WARMED: Once = Once::new();
    WARMED.call_once(|| {
        let dir = scratch_data_dir("busyaudit-warmup");
        let _ = mcphost::db::Db::open_with_cfg(&dir, mcphost::db::DbConfig::default());
        let _ = std::fs::remove_dir_all(&dir);
    });
}

/// Runs `f` with a scoped (thread-local, not global) tracing subscriber
/// installed and returns `f`'s result alongside everything it logged as
/// plain text -- safe to call from multiple tests running concurrently in
/// the same suite binary since [`tracing::subscriber::with_default`] is
/// thread-local, unlike `tracing::subscriber::set_global_default`.
pub fn capture_tracing<T>(f: impl FnOnce() -> T) -> (T, String) {
    warm_up_db_audit_callsites();
    let buf = CapturedLog::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(buf.clone())
        .with_ansi(false)
        .finish();
    let result = tracing::subscriber::with_default(subscriber, || {
        // `tracing`'s per-callsite "interest" cache is process-global and
        // memoized on first hit: if `db.rs`'s `db_audit` callsite fires on
        // some other thread before any subscriber is installed there (the
        // default no-op dispatch), tracing caches it as "never interested"
        // and this scoped subscriber never sees the event again, even
        // though `with_default` is active on this thread -- reproduces
        // under `cargo test`'s default multi-threaded runner, not when run
        // in isolation. Rebuilding while our subscriber is the current
        // dispatch re-evaluates interest against it before `f` runs --
        // `warm_up_db_audit_callsites` above is what makes this rebuild
        // reliable even when some other thread races to register one of
        // these call sites for the first time in this exact window.
        tracing::callsite::rebuild_interest_cache();
        f()
    });
    let text = String::from_utf8_lossy(&buf.0.lock().unwrap()).into_owned();
    (result, text)
}

pub fn scratch_data_dir(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "mcphost-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}
