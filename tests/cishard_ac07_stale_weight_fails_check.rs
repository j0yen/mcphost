//! contract: ci-test-partition.sh check fails on a recorded weight at 1.5x below its measured seconds
//! PRD-mcphost-ci-core-suites-sharded follow-up (2026-10-18, core shard over the
//! 720 s budget): the recorded weights sat at 341/375 s while CI measured 735/711 s
//! and `check` kept passing on the old numbers. `scripts/ci-suite-measured.json`
//! (written only by `scripts/ci-refresh-weights.sh`, from a CI log's
//! `suite=<name> measured=<s>s` lines) holds the max measured seconds, and `check`
//! fails, naming the suite, when measured >= 1.5 x recorded. It also fails, naming the
//! suite, when a weights key (the `list measured-required` emitter: minus lib/bin:*) has no
//! measured entry at all: a sandbox suite under its warn line used to record nothing, so
//! its wrong weight passed silently.

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

/// The suite keys that must carry a measured entry: THE emitter
/// (`ci-test-partition.sh list measured-required`, the weights file's keys minus `lib` and
/// `bin:*`), core AND sandbox. The refresh script and `check` read the same emitter.
fn required_suites_of(weights: &Path) -> Vec<String> {
    let o = Command::new(root().join("scripts/ci-test-partition.sh"))
        .args(["list", "measured-required"])
        .env("CI_SUITE_WEIGHTS_FILE", weights)
        .current_dir(root())
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).lines().map(str::to_owned).collect()
}

fn required_suites() -> Vec<String> {
    required_suites_of(&root().join("scripts/ci-suite-weights.json"))
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
    // every OTHER required suite reports too (the completeness rule), at 1 s
    let others: String = required_suites()
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
        .env("CI_REFRESH_ALLOW_LOCAL", "1")
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
    // a truncated log: every required suite reports EXCEPT suite_core_09 (the shard-3/5 case)
    let log = dir.join("ci.log");
    let body: String = required_suites()
        .iter()
        .filter(|k| *k != "suite_core_09")
        .map(|k| format!("2026-10-18T14:26:22Z suite={k} measured=1.00s recorded=1s\n"))
        .collect();
    std::fs::write(&log, body).unwrap();
    let o = Command::new(root().join("scripts/ci-refresh-weights.sh"))
        .arg(&log)
        .env("CI_REFRESH_ROOT", &dir)
        .env("CI_REFRESH_ALLOW_LOCAL", "1")
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&o.stderr).into_owned();
    let unchanged = std::fs::read(&w).unwrap() == w_before && std::fs::read(&m).unwrap() == m_before;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!o.status.success(), "a log lacking a core suite must fail");
    assert!(err.contains("suite_core_09"), "stderr names the missing suite: {err}");
    assert!(unchanged, "both json files stay byte-identical on a failed refresh");
}

#[test]
fn a_local_log_without_allow_local_is_refused_and_writes_nothing() {
    let dir = scratch("local");
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    let weights = json("scripts/ci-suite-weights.json");
    let (w, m) = (dir.join("scripts/ci-suite-weights.json"), dir.join("scripts/ci-suite-measured.json"));
    std::fs::write(&w, serde_json::to_string_pretty(&weights).unwrap()).unwrap();
    std::fs::write(&m, "{}").unwrap();
    let (w_before, m_before) = (std::fs::read(&w).unwrap(), std::fs::read(&m).unwrap());
    let log = dir.join("local.log");
    let body: String = required_suites()
        .iter()
        .map(|k| format!("2026-10-20T12:00:00Z suite={k} measured=1.00s recorded=1s\n"))
        .collect();
    std::fs::write(&log, body).unwrap();
    let o = Command::new(root().join("scripts/ci-refresh-weights.sh"))
        .arg(&log)
        .env("CI_REFRESH_ROOT", &dir)
        .env_remove("CI_REFRESH_ALLOW_LOCAL")
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&o.stderr).into_owned();
    let unchanged = std::fs::read(&w).unwrap() == w_before && std::fs::read(&m).unwrap() == m_before;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!o.status.success(), "a local log must be refused without CI_REFRESH_ALLOW_LOCAL=1");
    assert!(err.contains("CI_REFRESH_ALLOW_LOCAL"), "stderr names the override: {err}");
    assert!(unchanged, "both json files stay byte-identical on a refused refresh");
}

#[test]
fn a_log_missing_a_sandbox_suite_line_fails_naming_it_and_writes_nothing() {
    let dir = scratch("trunc-sandbox");
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    std::fs::create_dir_all(dir.join("tests")).unwrap();
    let weights = json("scripts/ci-suite-weights.json");
    let (w, m) = (dir.join("scripts/ci-suite-weights.json"), dir.join("scripts/ci-suite-measured.json"));
    std::fs::write(&w, serde_json::to_string_pretty(&weights).unwrap()).unwrap();
    std::fs::write(&m, "{}").unwrap();
    let (w_before, m_before) = (std::fs::read(&w).unwrap(), std::fs::read(&m).unwrap());
    // a sandbox shard that ran under its warn line and printed nothing (the defect): every
    // required suite reports EXCEPT one sandbox suite, derived from the emitter, never named here
    let required = required_suites();
    let silent = required.iter().find(|k| k.contains("sandbox")).expect("a sandbox suite is required").clone();
    let log = dir.join("ci.log");
    let body: String = required
        .iter()
        .filter(|k| **k != silent)
        .map(|k| format!("2026-10-20T14:26:22Z suite={k} measured=1.00s recorded=1s\n"))
        .collect();
    std::fs::write(&log, body).unwrap();
    let o = Command::new(root().join("scripts/ci-refresh-weights.sh"))
        .arg(&log)
        .env("CI_REFRESH_ROOT", &dir)
        .env("CI_REFRESH_ALLOW_LOCAL", "1")
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&o.stderr).into_owned();
    let unchanged = std::fs::read(&w).unwrap() == w_before && std::fs::read(&m).unwrap() == m_before;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!o.status.success(), "a log lacking a sandbox suite must fail");
    assert!(err.contains(&silent), "stderr names the missing sandbox suite {silent}: {err}");
    assert!(unchanged, "both json files stay byte-identical on a failed refresh");
}

#[test]
fn the_measured_required_emitter_is_the_weights_keys_minus_lib_and_bin() {
    let dir = scratch("emitter");
    let w = dir.join("weights.json");
    // lib and bin:* are in the weights file but never required; every other key is, core or sandbox
    std::fs::write(
        &w,
        r#"{"lib": 6, "bin:src/main.rs": 1, "suite_core_01": 3, "suite_sandbox_01": 4, "suite_env_sandbox_01": 5}"#,
    )
    .unwrap();
    let mut got = required_suites_of(&w);
    got.sort();
    assert_eq!(got, ["suite_core_01", "suite_env_sandbox_01", "suite_sandbox_01"]);
    // and on the committed weights it is exactly the keys minus lib / bin:*
    let weights = json("scripts/ci-suite-weights.json");
    let mut want: Vec<String> = weights
        .as_object()
        .unwrap()
        .keys()
        .filter(|k| *k != "lib" && !k.starts_with("bin:"))
        .cloned()
        .collect();
    want.sort();
    let mut got = required_suites();
    got.sort();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got, want);
}

#[test]
fn a_sandbox_suite_missing_from_the_measured_file_fails_check_naming_it() {
    let dir = scratch("unmeasured");
    let weights = json("scripts/ci-suite-weights.json");
    let silent = required_suites().into_iter().find(|k| k.contains("sandbox")).expect("a sandbox suite is required");
    let mut measured = json("scripts/ci-suite-measured.json");
    assert!(measured.get(&silent).is_some(), "the committed measured file carries {silent}");
    measured.as_object_mut().unwrap().remove(&silent);
    let (w, m) = (dir.join("weights.json"), dir.join("measured.json"));
    std::fs::write(&w, weights.to_string()).unwrap();
    std::fs::write(&m, measured.to_string()).unwrap();
    let o = check_with(&w, &m);
    let err = String::from_utf8_lossy(&o.stderr).into_owned();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!o.status.success(), "a weights key with no measured entry must fail check");
    assert!(err.contains(&format!("unmeasured suite: {silent} ")), "{err}");
}

#[test]
fn every_required_suite_has_a_measured_entry_in_the_committed_file() {
    let measured = json("scripts/ci-suite-measured.json");
    let missing: Vec<String> = required_suites().into_iter().filter(|k| measured.get(k).is_none()).collect();
    assert!(missing.is_empty(), "committed measured file lacks: {missing:?}");
}

#[test]
fn a_suite_made_only_of_new_files_is_seeded_provisional_and_the_refresh_clears_it() {
    // scripts/ci-suite-seed-selftest.sh builds its scratch repos under `mktemp -d` and proves: (a) a created suite
    // of only new files gets a measured entry + a `{pending, derived_s, since}` mark and `check` passes naming it,
    // (b) a measured value edited away from derived_s fails `check`, (c) a pending entry more than 3 first-parent
    // commits behind the default branch fails there, (d) the refresh removes the object (and exits 1 naming a
    // pending suite its log omits), (e) the committed tree has no generator drift, (g) a shallow clone is deepened,
    // (h) a resized suite is not marked.
    let o = Command::new("bash")
        .arg(root().join("scripts/ci-suite-seed-selftest.sh"))
        .env_remove("SEED_SELFTEST_SCRIPTS")
        .current_dir(root())
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(o.status.success(), "{out}\n{}", String::from_utf8_lossy(&o.stderr));
    assert!(out.contains("7 passed, 0 failed"), "{out}");
}
