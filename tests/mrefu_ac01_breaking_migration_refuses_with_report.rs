//! PRD-mcphost-migration-refusal-contract AC1 (P0) -- Given a temp db
//! migrated by a fixture `-- compat: breaking` migration that drops a column
//! and a stub previous binary failing `tools_call`, When `--check-compat
//! --report r.json` runs, Then exit is 3 and `r.json` validates against
//! `docs/compat-report.schema.json` with `migrations[0].compat ==
//! "breaking"`, `tables_changed[0].change == "columns_dropped"`,
//! `bypass.value == "<from>..<to>"`.

use crate::mrefu;

use std::fs;

use mrefu::{
    assert_valid_against_committed_schema, exec_sql, fixture_migration, run_check, scratch_dir,
    seed_live_db, stub_previous_failing_tools_call,
};

#[test]
fn mrefu_ac01_breaking_migration_exits_3_with_a_schema_valid_report() {
    let dir = scratch_dir("ac01");
    let db = seed_live_db(&dir);
    exec_sql(&db, "CREATE TABLE fixture_t (a TEXT, b TEXT);");
    let before = fs::read(&db).expect("read live db before");
    let fixture = fixture_migration(
        &dir,
        "0999_fixture_breaking.sql",
        "-- compat: breaking -- drops a column the previous release still reads\n\
         ALTER TABLE fixture_t DROP COLUMN b;\n",
    );
    let previous = stub_previous_failing_tools_call(&dir);

    let run = run_check(&dir, &db, &previous, Some(&fixture));

    assert_eq!(
        run.code,
        Some(3),
        "an incompatible migration must exit 3\nstdout: {}\nstderr: {}",
        run.stdout,
        run.stderr
    );
    // Tracing shares stdout; the contract line is the last one.
    assert_eq!(
        run.stdout.lines().last(),
        Some(format!("check-compat: incompatible report={}", run.report_path.display()).as_str()),
        "stdout ends with the one line naming the report"
    );

    let report = run.report();
    assert_valid_against_committed_schema(&report);
    assert_eq!(report["ok"], false);
    assert_eq!(report["step"], "tools_call");
    assert_eq!(report["migrations"][0]["name"], "0999_fixture_breaking.sql");
    assert_eq!(report["migrations"][0]["compat"], "breaking");
    assert_eq!(report["tables_changed"][0]["table"], "fixture_t");
    assert_eq!(report["tables_changed"][0]["change"], "columns_dropped");
    assert_eq!(report["tables_changed"][0]["columns"][0], "b");
    assert_eq!(report["from"], "0.106.0");
    assert_eq!(report["to"], env!("CARGO_PKG_VERSION"));
    assert_eq!(report["bypass"]["env"], "MCPHOST_DEPLOY_AUTHORIZE");
    assert_eq!(
        report["bypass"]["value"],
        format!("0.106.0..{}", env!("CARGO_PKG_VERSION"))
    );

    // The remedy names the backup first and the bypass second, no file path.
    let remedy = report["remedy"].as_str().expect("remedy is text");
    let backup = remedy.find("mcphost-deploy backup").expect("remedy names the backup");
    let lift = remedy
        .find(&format!("MCPHOST_DEPLOY_AUTHORIZE=0.106.0..{}", env!("CARGO_PKG_VERSION")))
        .expect("remedy names the bypass");
    assert!(backup < lift, "backup before bypass: {remedy}");
    assert!(!remedy.contains('/'), "remedy names no path: {remedy}");

    assert_eq!(before, fs::read(&db).expect("read live db after"), "the live database is untouched");
    let _ = fs::remove_dir_all(&dir);
}
