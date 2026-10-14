//! PRD-mcphost-migration-refusal-contract AC2 (P0) -- Given a compatible
//! migration (`-- compat: previous`, one nullable column), When the check
//! runs, Then exit 0, `ok: true`, `tables_changed[0].change ==
//! "columns_added"`, and `bypass` is absent.

use crate::mrefu;

use mrefu::{
    assert_valid_against_committed_schema, exec_sql, fixture_migration, mcphost_bin, run_check,
    scratch_dir, seed_live_db,
};

#[test]
fn mrefu_ac02_compatible_migration_exits_0_with_ok_true_and_no_bypass() {
    let dir = scratch_dir("ac02");
    let db = seed_live_db(&dir);
    exec_sql(&db, "CREATE TABLE fixture_t (a TEXT);");
    let fixture = fixture_migration(
        &dir,
        "0998_fixture_previous.sql",
        "-- compat: previous -- additive nullable column; the previous release never queries it\n\
         ALTER TABLE fixture_t ADD COLUMN note TEXT;\n",
    );

    // The current binary is its own "previous" release: it serves the
    // migrated copy fine, exactly what a compatible migration looks like.
    let run = run_check(&dir, &db, &mcphost_bin(), Some(&fixture));

    assert_eq!(
        run.code,
        Some(0),
        "a compatible migration must exit 0\nstdout: {}\nstderr: {}",
        run.stdout,
        run.stderr
    );
    assert_eq!(
        run.stdout.lines().last(),
        Some(format!("check-compat: ok report={}", run.report_path.display()).as_str()),
        "stdout ends with the one line naming the report"
    );

    let report = run.report();
    assert_valid_against_committed_schema(&report);
    assert_eq!(report["ok"], true);
    assert_eq!(report["tables_changed"].as_array().map(Vec::len), Some(1), "one table changed");
    assert_eq!(report["tables_changed"][0]["table"], "fixture_t");
    assert_eq!(report["tables_changed"][0]["change"], "columns_added");
    assert_eq!(report["tables_changed"][0]["columns"][0], "note");
    assert_eq!(report["migrations"][0]["name"], "0998_fixture_previous.sql");
    assert_eq!(report["migrations"][0]["compat"], "previous");
    for key in ["step", "detail", "reason", "remedy"] {
        assert!(report[key].is_null(), "{key} is null on ok: true, got {}", report[key]);
    }
    assert!(
        report.get("bypass").is_none(),
        "bypass is absent on ok: true, got {}",
        report["bypass"]
    );
    let _ = std::fs::remove_dir_all(&dir);
}
