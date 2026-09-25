//! Shared tracing-capture helper for the SQLite busy-timeout-audit AC
//! files that read startup pragma-audit log lines (AC1, AC2) -- not a
//! crate module, pulled in per-file with `#[path = "support/busyaudit.rs"]
//! mod busyaudit;`, same convention as `tests/support/host.rs`.

use std::sync::{Arc, Mutex};

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

/// Runs `f` with a scoped (thread-local, not global) tracing subscriber
/// installed and returns `f`'s result alongside everything it logged as
/// plain text -- safe to call from multiple tests running concurrently in
/// the same suite binary since [`tracing::subscriber::with_default`] is
/// thread-local, unlike `tracing::subscriber::set_global_default`.
pub fn capture_tracing<T>(f: impl FnOnce() -> T) -> (T, String) {
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
        // dispatch re-evaluates interest against it before `f` runs.
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
