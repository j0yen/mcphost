//! PRD-mcphost-migration-refusal-contract AC4 (P0) -- Given a previous
//! binary that exits 1 on start, When the check runs, Then exit 4, `step ==
//! "previous-up"`, and `tables_changed` is still populated.

use crate::mrefu;

use std::path::PathBuf;

use mrefu::{
    assert_valid_against_committed_schema, exec_sql, fixture_migration, run_check, scratch_dir,
    seed_live_db,
};

#[test]
fn mrefu_ac04_previous_binary_that_exits_1_reports_previous_up_with_the_diff() {
    let dir = scratch_dir("ac04");
    let db = seed_live_db(&dir);
    exec_sql(&db, "CREATE TABLE fixture_t (a TEXT);");
    let fixture = fixture_migration(
        &dir,
        "0997_fixture_additive.sql",
        "-- compat: previous -- additive nullable column\n\
         ALTER TABLE fixture_t ADD COLUMN note TEXT;\n",
    );
    // `false` exits 1 immediately: the previous release never comes up.
    let exits_1 = ["/bin/false", "/usr/bin/false"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.is_file())
        .expect("a `false` binary");

    let run = run_check(&dir, &db, &exits_1, Some(&fixture));

    assert_eq!(
        run.code,
        Some(4),
        "a previous binary that cannot start must exit 4\nstdout: {}\nstderr: {}",
        run.stdout,
        run.stderr
    );
    assert_eq!(
        run.stdout.lines().last(),
        Some(format!("check-compat: previous-up-failed report={}", run.report_path.display()).as_str()),
        "stdout ends with the one line naming the report"
    );

    let report = run.report();
    assert_valid_against_committed_schema(&report);
    assert_eq!(report["ok"], false);
    assert_eq!(report["step"], "previous-up");
    assert_eq!(report["tables_changed"][0]["table"], "fixture_t");
    assert_eq!(report["tables_changed"][0]["change"], "columns_added");
    assert_eq!(report["migrations"][0]["name"], "0997_fixture_additive.sql");
    assert_eq!(report["bypass"]["env"], "MCPHOST_DEPLOY_AUTHORIZE");
    let _ = std::fs::remove_dir_all(&dir);
}
