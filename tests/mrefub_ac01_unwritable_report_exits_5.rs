//! PRD-mcphost-migration-refusal-contract-b AC1 (P0) -- Given `--report`
//! pointing at an unwritable path, When the check runs, Then exit 5 and the
//! status line names the path; no partial file remains.

use crate::mrefu::{exec_sql, mcphost_bin, scratch_dir, seed_live_db, stub_previous_failing_tools_call};

use std::process::Command;

#[test]
fn mrefub_ac01_unwritable_report_path_exits_5_and_names_the_path() {
    let dir = scratch_dir("b-ac01");
    let db = seed_live_db(&dir);
    exec_sql(&db, "CREATE TABLE fixture_t (a TEXT);");
    let stub = stub_previous_failing_tools_call(&dir);

    // Two unwritable shapes: a parent directory that does not exist, and a
    // target that is an existing directory (the write succeeds into the tmp
    // file, the rename fails -- the tmp file must be cleaned up).
    let missing_parent = dir.join("no-such-dir").join("r.json");
    let dir_target = dir.join("target-is-a-dir.json");
    std::fs::create_dir_all(&dir_target).expect("dir target");

    for report in [&missing_parent, &dir_target] {
        let out = Command::new(mcphost_bin())
            .arg("migrate")
            .arg("--check-compat")
            .arg("--previous")
            .arg(&stub)
            .arg("--db")
            .arg(&db)
            .arg("--report")
            .arg(report)
            .output()
            .expect("run check");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_eq!(out.status.code(), Some(5), "stdout: {stdout}\nstderr: {}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(
            stdout.lines().last(),
            Some(format!("check-compat: report-unwritable report={}", report.display()).as_str()),
            "the status line names the path"
        );
    }

    assert!(!missing_parent.exists(), "no file at the unwritable path");
    assert!(dir_target.is_dir(), "the existing directory is untouched");
    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "partial files remain: {leftovers:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
