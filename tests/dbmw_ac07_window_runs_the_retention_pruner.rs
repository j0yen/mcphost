//! PRD-mcphost-db-maintenance-window AC7 (P1): with the retention pruner last
//! run 21 h ago, the window runs its daily pass between the two maintenance
//! rows; with a recent run it does not.

use crate::common;

use mcphost::heal::HealAction;
use mcphost::maintenance::run_window;
use rusqlite::Connection;

fn prune_starts(path: &std::path::Path) -> Vec<(i64, i64)> {
    Connection::open(path)
        .unwrap()
        .prepare("SELECT started_unix, finished_unix FROM prune_log ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn seed_prune(path: &std::path::Path, started: i64) {
    Connection::open(path)
        .unwrap()
        .execute(
            "INSERT INTO prune_log (started_unix, finished_unix, ok, error, deleted_json) VALUES (?1, ?1, 1, NULL, '{}')",
            [started],
        )
        .unwrap();
}

#[tokio::test]
async fn pruner_last_run_21h_ago_runs_inside_the_window() {
    let (state, dir) = common::bare_app_state().await;
    let path = dir.0.join("mcphost.db");
    let now = mcphost::state::now_unix();
    seed_prune(&path, now - 21 * 3600);

    let report = run_window(&state.db, now, false).await.expect("window");
    let db_row = report.rows.iter().find(|r| r.action == HealAction::DbMaintenance).unwrap();
    let log_row = report.rows.iter().find(|r| r.action == HealAction::LogRotate).unwrap();

    let runs = prune_starts(&path);
    assert_eq!(runs.len(), 2, "one seeded run plus the window's: {runs:?}");
    let (started, finished) = runs[1];
    assert!(db_row.at <= started, "pruner started {started} before the first row {}", db_row.at);
    assert!(finished <= log_row.at, "pruner finished {finished} after the second row {}", log_row.at);
    assert!(db_row.id < log_row.id);
}

#[tokio::test]
async fn pruner_that_ran_recently_is_left_alone() {
    let (state, dir) = common::bare_app_state().await;
    let path = dir.0.join("mcphost.db");
    let now = mcphost::state::now_unix();
    seed_prune(&path, now - 3600);

    run_window(&state.db, now, false).await.expect("window");
    assert_eq!(prune_starts(&path).len(), 1, "no second prune cycle");
}
