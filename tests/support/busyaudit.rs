//! Shared tracing-capture helper for the SQLite busy-timeout-audit AC
//! files that read startup pragma-audit log lines (AC1, AC2) -- not a
//! crate module, pulled in per-file with `#[path = "support/busyaudit.rs"]
//! mod busyaudit;`, same convention as `tests/support/host.rs`.

// Not every suite binary that includes this file calls every helper (e.g.
// scratch_data_dir is unused where only capture_tracing is).
#![allow(dead_code)]

/// Runs `f` with this thread's tracing events captured as plain text and
/// returns `f`'s result alongside everything it logged. Goes through
/// `common::tracecap`: one process-global subscriber per test binary, with
/// events routed to the emitting thread's capture, so concurrent callers in
/// the same suite binary neither cross-read nor invalidate each other's
/// callsite interest (the old scoped `with_default` +
/// `rebuild_interest_cache()` + process-wide lock + warm-up `Db::open`
/// dance dropped `db_audit` lines when a rebuild landed mid-`f`).
pub fn capture_tracing<T>(f: impl FnOnce() -> T) -> (T, String) {
    let capture = crate::common::tracecap::capture_text();
    let result = f();
    (result, capture.finish())
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
