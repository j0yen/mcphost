//! PRD-mcphost-db-maintenance-window AC3 (P0): a `.backup-lock` younger than
//! 2 h makes both rows `refused` ("backup in progress"), checkpoints nothing
//! and schedules the next attempt 3600 s later; a 3-h-old lock is ignored
//! with a `stale backup lock ignored` tracing line and the actions run.

use crate::busyaudit::capture_tracing;
use crate::common;

use std::time::{Duration, SystemTime};

use mcphost::heal::{HealAction, HealOutcome};
use mcphost::maintenance::{BACKUP_LOCK_FILE, run_window};
use rusqlite::Connection;

fn make_lock(dir: &std::path::Path, age: Duration) {
    let f = std::fs::File::create(dir.join(BACKUP_LOCK_FILE)).expect("create lock");
    f.set_modified(SystemTime::now() - age).expect("age the lock");
}

#[tokio::test]
async fn fresh_lock_refuses_both_actions_and_retries_in_an_hour() {
    let (state, dir) = common::bare_app_state().await;
    crate::dbmw_ac02_db_maintenance_truncates_wal_and_vacuums::seed_wal_and_freelist(&dir.0.join("mcphost.db"));
    make_lock(&dir.0, Duration::from_secs(600));
    let wal_before = std::fs::metadata(dir.0.join("mcphost.db-wal")).expect("wal").len();
    assert!(wal_before > 0);

    let now = mcphost::state::now_unix();
    let report = run_window(&state.db, now, false).await.expect("window");
    assert_eq!(report.rows.len(), 2);
    assert_eq!(report.rows[0].action, HealAction::DbMaintenance);
    assert_eq!(report.rows[1].action, HealAction::LogRotate);
    for row in &report.rows {
        assert_eq!(row.outcome, HealOutcome::Refused, "{row:?}");
        assert_eq!(row.after["reason"], "backup in progress");
        let stored = state.db.find_heal_row(row.id).await.unwrap().expect("row");
        assert_eq!(stored.outcome, HealOutcome::Refused);
        assert_eq!(stored.after["reason"], "backup in progress");
    }
    assert_eq!(report.next_unix, now + 3600);

    // Nothing was checkpointed or vacuumed: the WAL file is not truncated and
    // the freelist is intact.
    let wal_after = std::fs::metadata(dir.0.join("mcphost.db-wal")).expect("wal").len();
    assert!(wal_after >= wal_before, "wal shrank {wal_before} -> {wal_after}");
    let freelist: i64 = Connection::open(dir.0.join("mcphost.db"))
        .unwrap()
        .query_row("PRAGMA freelist_count", [], |r| r.get(0))
        .unwrap();
    assert!(freelist >= 1000, "freelist {freelist}");
}

#[test]
fn stale_lock_is_ignored_and_the_actions_run() {
    // A sync test with its own runtime, so `capture_tracing` (which serializes
    // its interest-cache rebuild against the other tracing tests in this
    // binary) can wrap the whole window.
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
    let (state, dir) = rt.block_on(common::bare_app_state());
    make_lock(&dir.0, Duration::from_secs(3 * 3600));

    let (report, log) = capture_tracing(|| {
        rt.block_on(run_window(&state.db, mcphost::state::now_unix(), false)).expect("window")
    });
    for row in &report.rows {
        assert_eq!(row.outcome, HealOutcome::Ok, "{row:?}");
    }
    assert!(log.contains("stale backup lock ignored"), "log was:\n{log}");
}
