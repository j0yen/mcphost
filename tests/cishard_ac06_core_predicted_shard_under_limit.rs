//! contract: the heaviest predicted core shard stays under its limit
//! PRD-mcphost-ci-core-suites-sharded AC6 -- from the recorded weights `check`
//! computes the heaviest predicted core shard and it is <= 540 s (the CI warn line; the recorded weights are max measured hosted-runner seconds). (The other
//! half of AC6, a shard run printing build wall, run wall and per-suite
//! measured vs recorded seconds, is pinned in
//! cishard_ac03_run_wall_budget_and_readout.rs beside the sandbox readout.)

use std::path::PathBuf;
use std::process::{Command, Output};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
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
fn check_reports_a_predicted_heaviest_core_shard_within_540_seconds() {
    let checked = out(&["check"]);
    let tail = checked.lines().last().unwrap_or_default();
    let n: f64 = tail
        .split("predicted heaviest core shard ")
        .nth(1)
        .and_then(|r| r.split('s').next())
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| panic!("no core prediction in: {tail}"));
    assert!(n <= 540.0, "predicted heaviest core shard {n}s > 540s");
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
    assert!(heaviest <= 540.0, "heaviest predicted core shard {heaviest}s");
    let checked = out(&["check"]);
    assert!(
        checked.contains(&format!("predicted heaviest core shard {heaviest}s")),
        "check's figure must equal the independent sum {heaviest}s: {checked}"
    );
}

#[test]
fn a_weight_over_the_limit_fails_check_naming_the_figure() {
    let dir = std::env::temp_dir().join(format!("cishard-core-ac06-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let text = std::fs::read_to_string(root().join("scripts/ci-suite-weights.json")).unwrap();
    let mut w: serde_json::Value = serde_json::from_str(&text).unwrap();
    let first = out(&["list", "core"]).lines().next().unwrap().to_string();
    w[first.as_str()] = serde_json::json!(999);
    let file = dir.join("weights.json");
    std::fs::write(&file, w.to_string()).unwrap();
    let o = Command::new(root().join("scripts/ci-test-partition.sh"))
        .arg("check")
        .env("CI_SUITE_WEIGHTS_FILE", &file)
        .current_dir(root())
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("predicted heaviest core shard is 999s"), "{err}");
}
