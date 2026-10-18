//! contract: the heaviest predicted core shard stays under its limit
//! PRD-mcphost-ci-core-suites-sharded AC6 -- from the recorded weights `check`
//! computes the heaviest predicted core shard and it is <= 720 s (CORE_FAIL_ABOVE,
//! the CI fail line; above 540 s, CORE_WARN_AT, it only WARNS on stderr, exit 0; both
//! limits are read from `ci-test-partition.sh limits core`). (The other
//! half of AC6, a shard run printing build wall, run wall and per-suite
//! measured vs recorded seconds, is pinned in
//! cishard_ac03_run_wall_budget_and_readout.rs beside the sandbox readout.)

use std::path::PathBuf;
use std::process::{Command, Output};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// A core limit from `ci-test-partition.sh limits core` (the one definition the CI
/// run-wall script reads too).
fn limit(name: &str) -> f64 {
    let o = Command::new(root().join("scripts/ci-test-partition.sh"))
        .args(["limits", "core"])
        .current_dir(root())
        .output()
        .expect("run ci-test-partition.sh limits core");
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let text = String::from_utf8_lossy(&o.stdout).into_owned();
    let v: Vec<f64> = text.split_whitespace().map(|v| v.parse().unwrap()).collect();
    match name {
        "CORE_WARN_AT" => v[0],
        "CORE_FAIL_ABOVE" => v[1],
        _ => panic!("unknown limit {name}"),
    }
}

fn partition(args: &[&str]) -> Output {
    Command::new(root().join("scripts/ci-test-partition.sh"))
        .args(args)
        .current_dir(root())
        .output()
        .expect("run scripts/ci-test-partition.sh")
}

fn out(args: &[&str]) -> String {
    let o = partition(args);
    assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).into_owned()
}

#[test]
fn check_reports_a_predicted_heaviest_core_shard_within_the_fail_line() {
    let checked = out(&["check"]);
    let tail = checked.lines().last().unwrap_or_default();
    let n: f64 = tail
        .split("predicted heaviest core shard ")
        .nth(1)
        .and_then(|r| r.split('s').next())
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| panic!("no core prediction in: {tail}"));
    assert!(n <= limit("CORE_FAIL_ABOVE"), "predicted heaviest core shard {n}s > the fail line");
}

#[test]
fn the_prediction_matches_an_independent_sum_of_the_weights() {
    let text = std::fs::read_to_string(root().join("scripts/ci-suite-weights.json")).unwrap();
    let w: serde_json::Value = serde_json::from_str(&text).unwrap();
    let total: usize = out(&["shards", "core"]).trim().parse().unwrap();
    let mut heaviest = 0.0f64;
    for n in 1..=total {
        let flags = out(&["core-shard", &n.to_string(), &total.to_string()]);
        let sum: f64 = flags
            .split_whitespace()
            .collect::<Vec<_>>()
            .windows(2)
            .filter(|p| p[0] == "--test")
            .map(|p| w[p[1]].as_f64().unwrap_or_else(|| panic!("no weight for {}", p[1])))
            .sum();
        heaviest = heaviest.max(sum);
    }
    assert!(heaviest <= limit("CORE_FAIL_ABOVE"), "heaviest predicted core shard {heaviest}s");
    let checked = out(&["check"]);
    assert!(
        checked.contains(&format!("predicted heaviest core shard {heaviest}s")),
        "check's figure must equal the independent sum {heaviest}s: {checked}"
    );
}

/// `check` with the first core suite weighted `weight` (scratch weights file).
fn check_with_first_core_weight(tag: &str, weight: u32) -> Output {
    let dir = std::env::temp_dir().join(format!("cishard-core-ac06-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let text = std::fs::read_to_string(root().join("scripts/ci-suite-weights.json")).unwrap();
    let mut w: serde_json::Value = serde_json::from_str(&text).unwrap();
    let first = out(&["list", "core"]).lines().next().unwrap().to_string();
    w[first.as_str()] = serde_json::json!(weight);
    let file = dir.join("weights.json");
    std::fs::write(&file, w.to_string()).unwrap();
    let o = Command::new(root().join("scripts/ci-test-partition.sh"))
        .arg("check")
        .env("CI_SUITE_WEIGHTS_FILE", &file)
        .current_dir(root())
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    o
}

#[test]
fn a_weight_over_the_limit_fails_check_naming_the_figure() {
    let o = check_with_first_core_weight("999", 999);
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("predicted heaviest core shard is 999s"), "{err}");
}

#[test]
fn a_prediction_between_the_warn_and_fail_lines_warns_and_passes() {
    // 600 s is above the 540 s warn line and below the 720 s fail line (ryzen7 runs
    // 410 and 404 were gate-blocked at a predicted 608 s that CI runs fine).
    let (warn, fail) = (limit("CORE_WARN_AT"), limit("CORE_FAIL_ABOVE"));
    assert!(warn < 600.0 && 600.0 <= fail, "600 must sit between {warn} and {fail}");
    let o = check_with_first_core_weight("600", 600);
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(o.status.success(), "a 600 s prediction must pass: {err}");
    // The packer may place another core suite in the same shard, so the figure is
    // 600 s plus whatever shares it: assert it is at least 600 and still between the lines.
    let figure: f64 = err
        .split("WARNING predicted heaviest core shard is ")
        .nth(1)
        .and_then(|rest| rest.split('s').next())
        .and_then(|n| n.trim().parse().ok())
        .unwrap_or_else(|| panic!("the warning must name the shard figure: {err}"));
    assert!(
        figure >= 600.0 && figure > warn && figure <= fail && err.contains(&format!("> {warn}s warn line")),
        "the warning must name a figure >= 600 between the warn and fail lines, and the warn line: {err}"
    );
}

#[test]
fn a_prediction_above_the_fail_line_fails() {
    assert!(limit("CORE_FAIL_ABOVE") < 730.0);
    let o = check_with_first_core_weight("730", 730);
    assert!(!o.status.success(), "a 730 s prediction must fail");
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("predicted heaviest core shard is 730s"), "{err}");
}
