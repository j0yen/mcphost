//! contract: ci-test-partition.sh check fails on a recorded weight at 1.5x below its measured seconds
//! PRD-mcphost-ci-core-suites-sharded follow-up (2026-10-18, core shard over the
//! 720 s budget): the recorded weights sat at 341/375 s while CI measured 735/711 s
//! and `check` kept passing on the old numbers. `scripts/ci-suite-measured.json`
//! (written only by `scripts/ci-refresh-weights.sh`, from a CI log's
//! `suite=<name> measured=<s>s` lines) holds the max measured seconds, and `check`
//! fails, naming the suite, when measured >= 1.5 x recorded.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cishard-ac07-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn json(rel: &str) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(root().join(rel)).unwrap()).unwrap()
}

/// The CORE suite keys of the weights file (generated, same rule as the refresh script:
/// not `lib`, `bin:*`, or a sandbox suite), each with a plausible `suite=... measured=` line.
fn core_suites(weights: &serde_json::Value) -> Vec<String> {
    weights
        .as_object()
        .unwrap()
        .keys()
        .filter(|k| *k != "lib" && !k.starts_with("bin:") && !k.contains("sandbox"))
        .cloned()
        .collect()
}

fn check_with(weights: &Path, measured: &Path) -> Output {
    Command::new(root().join("scripts/ci-test-partition.sh"))
        .arg("check")
        .env("CI_SUITE_WEIGHTS_FILE", weights)
        .env("CI_SUITE_MEASURED_FILE", measured)
        .current_dir(root())
        .output()
        .unwrap()
}

#[test]
fn the_committed_measured_file_passes_check() {
    let o = Command::new(root().join("scripts/ci-test-partition.sh"))
        .arg("check")
        .current_dir(root())
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
}

#[test]
fn measured_at_one_and_a_half_times_recorded_fails_check_naming_the_suite() {
    let dir = scratch("stale");
    let weights = json("scripts/ci-suite-weights.json");
    let rec = weights["suite_core_08"].as_f64().unwrap();
    let mut measured = json("scripts/ci-suite-measured.json");
    measured["suite_core_08"] = serde_json::json!(rec * 1.5);
    let (w, m) = (dir.join("weights.json"), dir.join("measured.json"));
    std::fs::write(&w, weights.to_string()).unwrap();
    std::fs::write(&m, measured.to_string()).unwrap();
    let o = check_with(&w, &m);
    let err = String::from_utf8_lossy(&o.stderr).into_owned();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!o.status.success(), "a 1.5x-stale pair must fail check");
    assert!(err.contains("stale weight: suite_core_08 measured"), "{err}");
}

#[test]
fn a_fresh_pair_just_under_the_ratio_passes_check() {
    let dir = scratch("fresh");
    let weights = json("scripts/ci-suite-weights.json");
    let rec = weights["suite_core_08"].as_f64().unwrap();
    let mut measured = json("scripts/ci-suite-measured.json");
    measured["suite_core_08"] = serde_json::json!(rec * 1.49);
    let (w, m) = (dir.join("weights.json"), dir.join("measured.json"));
    std::fs::write(&w, weights.to_string()).unwrap();
    std::fs::write(&m, measured.to_string()).unwrap();
    let o = check_with(&w, &m);
    let err = String::from_utf8_lossy(&o.stderr).into_owned();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(o.status.success(), "{err}");
}

#[test]
fn a_missing_measured_file_fails_check() {
    let dir = scratch("missing");
    let o = check_with(&root().join("scripts/ci-suite-weights.json"), &dir.join("nope.json"));
    let err = String::from_utf8_lossy(&o.stderr).into_owned();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!o.status.success());
    assert!(err.contains("ci-suite-measured.json is missing"), "{err}");
}

#[test]
fn the_refresh_script_is_the_writer_that_makes_a_stale_pair_pass_again() {
    let dir = scratch("refresh");
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    std::fs::create_dir_all(dir.join("tests")).unwrap();
    let weights = json("scripts/ci-suite-weights.json");
    let stale = weights["suite_core_08"].as_f64().unwrap() * 2.0;
    std::fs::write(dir.join("scripts/ci-suite-weights.json"), weights.to_string()).unwrap();
    std::fs::write(dir.join("scripts/ci-suite-measured.json"), "{}").unwrap();
    let log = dir.join("ci.log");
    // every OTHER core suite reports too (the completeness rule), at 1 s
    let others: String = core_suites(&weights)
        .iter()
        .filter(|k| *k != "suite_core_08")
        .map(|k| format!("2026-10-18T14:26:22Z suite={k} measured=1.00s recorded=1s\n"))
        .collect();
    std::fs::write(
        &log,
        format!(
            "2026-10-18T14:26:20Z \x1b[0msuite=bin:src/main.rs measured=0.00s recorded=unrecorded\n\
             2026-10-18T14:26:20Z suite=suite_core_08 measured={stale:.2}s recorded=291s\n\
             2026-10-18T14:26:21Z suite=suite_core_08 measured=1.00s recorded=291s\n{others}"
        ),
    )
    .unwrap();
    let o = Command::new(root().join("scripts/ci-refresh-weights.sh"))
        .arg(&log)
        .env("CI_REFRESH_ROOT", &dir)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let w: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("scripts/ci-suite-weights.json")).unwrap()).unwrap();
    let m: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("scripts/ci-suite-measured.json")).unwrap()).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(w["suite_core_08"].as_f64().unwrap(), stale.ceil(), "recorded follows the max measured");
    assert_eq!(m["suite_core_08"].as_f64().unwrap(), (stale * 100.0).round() / 100.0, "max over the log, bin:* ignored");
    assert!(m.get("bin:src/main.rs").is_none());
}

#[test]
fn a_log_missing_a_core_suite_line_fails_naming_it_and_writes_nothing() {
    let dir = scratch("trunc");
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    std::fs::create_dir_all(dir.join("tests")).unwrap();
    let weights = json("scripts/ci-suite-weights.json");
    let (w, m) = (dir.join("scripts/ci-suite-weights.json"), dir.join("scripts/ci-suite-measured.json"));
    std::fs::write(&w, serde_json::to_string_pretty(&weights).unwrap()).unwrap();
    std::fs::write(&m, "{}").unwrap();
    let (w_before, m_before) = (std::fs::read(&w).unwrap(), std::fs::read(&m).unwrap());
    // a truncated log: every core suite reports EXCEPT suite_core_09 (the shard-3/5 case)
    let log = dir.join("ci.log");
    let body: String = core_suites(&weights)
        .iter()
        .filter(|k| *k != "suite_core_09")
        .map(|k| format!("2026-10-18T14:26:22Z suite={k} measured=1.00s recorded=1s\n"))
        .collect();
    std::fs::write(&log, body).unwrap();
    let o = Command::new(root().join("scripts/ci-refresh-weights.sh"))
        .arg(&log)
        .env("CI_REFRESH_ROOT", &dir)
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&o.stderr).into_owned();
    let unchanged = std::fs::read(&w).unwrap() == w_before && std::fs::read(&m).unwrap() == m_before;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!o.status.success(), "a log lacking a core suite must fail");
    assert!(err.contains("suite_core_09"), "stderr names the missing suite: {err}");
    assert!(unchanged, "both json files stay byte-identical on a failed refresh");
}
