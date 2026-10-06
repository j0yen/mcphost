//! PRD-mcphost-sqlite-busy-timeout-audit
//! AC1 — Given the server starts with default config, When it opens its
//! connections, Then the log shows one audit line per role with
//! `busy_timeout=5000 journal_mode=wal synchronous=normal foreign_keys=1`
//! read back from the connection.

use crate::busyaudit;

use busyaudit::{capture_tracing, scratch_data_dir};
use mcphost::db::{Db, DbConfig};
use std::time::{Duration, Instant};

#[test]
fn startup_audit_logs_one_line_per_role_with_default_pragmas() {
    let expected = "busy_timeout=5000 journal_mode=wal synchronous=normal foreign_keys=1";

    // `capture_tracing`'s `CAPTURE_LOCK` (see tests/support/busyaudit.rs)
    // only serializes calls that go through `capture_tracing` itself. It
    // cannot stop an unrelated, uncoordinated
    // `tracing::callsite::rebuild_interest_cache()` call elsewhere in this
    // ~1000-test consolidated suite binary --
    // `sessbind_ac10_request_log_records_the_upgraded_status.rs` calls it
    // directly on its own thread, not through `capture_tracing` -- from
    // landing in the brief real-disk-I/O gap `Db::open_with_cfg` leaves
    // between its `role=server` and `role=prune` `db_audit` lines and
    // flipping that callsite's process-global cached `Interest` to "never"
    // in between. That is a genuine, rare scheduling race rather than a
    // logic bug in this test, so retry (bounded, not unbounded) rather
    // than failing outright on the first loss: each retry re-takes
    // `CAPTURE_LOCK` and re-runs this test's own
    // `rebuild_interest_cache()` immediately before its two real audit
    // lines, and only loses the race again if another such rebuild lands
    // in that same handful-of-microseconds window a second time --
    // vanishingly likely inside the bound below. The two assertions below
    // are unchanged: still exactly one line per role with the default
    // pragmas, never weakened.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut log;
    loop {
        let data_dir = scratch_data_dir("busyaudit-ac01");
        let (db, captured) = capture_tracing(|| Db::open_with_cfg(&data_dir, DbConfig::default()));
        let _db = db.expect("Db::open with default config");
        let _ = std::fs::remove_dir_all(&data_dir);
        log = captured;

        let got_both = log.contains(&format!("role=server {expected}"))
            && log.contains(&format!("role=prune {expected}"));
        if got_both || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    assert!(
        log.contains(&format!("role=server {expected}")),
        "expected a role=server audit line in:\n{log}"
    );
    assert!(
        log.contains(&format!("role=prune {expected}")),
        "expected a role=prune audit line in:\n{log}"
    );
}
