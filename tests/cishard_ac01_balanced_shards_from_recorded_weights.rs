//! contract: sandbox shards balance the recorded scripts/ci-suite-weights.json
//! PRD-mcphost-ci-sandbox-shards-balanced AC1 -- sandbox suites are split into
//! shards by recorded cost (`scripts/ci-suite-weights.json`), heaviest first
//! into the lightest shard, and a suite without a weight fails `check`.

use std::path::PathBuf;
use std::process::{Command, Output};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn partition(args: &[&str], weights: Option<&PathBuf>) -> Output {
    let mut cmd = Command::new(root().join("scripts/ci-test-partition.sh"));
    cmd.args(args).current_dir(root());
    if let Some(w) = weights {
        cmd.env("CI_SUITE_WEIGHTS_FILE", w);
    }
    cmd.output().expect("run scripts/ci-test-partition.sh")
}

fn stdout(o: &Output) -> String {
    assert!(
        o.status.success(),
        "exit {:?}\nstderr: {}",
        o.status.code(),
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn suites_of(flags: &str) -> Vec<String> {
    flags
        .split_whitespace()
        .collect::<Vec<_>>()
        .windows(2)
        .filter(|w| w[0] == "--test")
        .map(|w| w[1].to_string())
        .collect()
}

#[test]
fn the_committed_weights_split_into_three_total_disjoint_shards() {
    let weights = root().join("scripts/ci-suite-weights.json");
    let text = std::fs::read_to_string(&weights).expect("weights file exists");
    let json: serde_json::Value = serde_json::from_str(&text).expect("weights file is JSON");
    assert_eq!(json["lib"], 6);
    assert_eq!(json["suite_env_sandbox_01"], 7);
    assert_eq!(json["suite_sandbox_01"], 110);
    assert_eq!(json["suite_sandbox_02"], 146);

    let shards: Vec<String> = (1..=3)
        .map(|n| stdout(&partition(&["sandbox-shard", &n.to_string(), "3"], None)))
        .collect();

    let mut union: Vec<String> = shards.iter().flat_map(|s| suites_of(s)).collect();
    union.sort();
    let mut listed: Vec<String> = stdout(&partition(&["list", "sandbox"], None))
        .lines()
        .map(str::to_string)
        .collect();
    listed.sort();
    assert_eq!(union, listed, "shards must be total and disjoint over the sandbox suites");

    let heavy: Vec<&String> = shards
        .iter()
        .filter(|s| suites_of(s).contains(&"suite_sandbox_02".to_string()))
        .collect();
    assert_eq!(heavy.len(), 1);
    assert_eq!(
        suites_of(heavy[0]),
        vec!["suite_sandbox_02".to_string()],
        "suite_sandbox_02 must be alone on its shard"
    );
    assert!(!heavy[0].contains("--lib"), "--lib must not sit with the heaviest shard");

    let with_lib: Vec<&String> = shards.iter().filter(|s| s.contains("--lib")).collect();
    assert_eq!(with_lib.len(), 1, "--lib runs exactly once");
    // The two light suites (env + the generated contract suite) share the --lib shard.
    let mut light = suites_of(with_lib[0]);
    light.sort();
    assert_eq!(
        light,
        vec!["suite_contracts_sandbox_01".to_string(), "suite_env_sandbox_01".to_string()]
    );
}

#[test]
fn check_reports_a_predicted_heaviest_shard_within_160_seconds() {
    let out = stdout(&partition(&["check"], None));
    let tail = out.lines().last().unwrap_or_default();
    let n: f64 = tail
        .split("predicted heaviest shard ")
        .nth(1)
        .and_then(|r| r.split('s').next())
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| panic!("no prediction in: {tail}"));
    assert!(n <= 160.0, "predicted heaviest shard {n}s > 160s");
}

#[test]
fn a_suite_without_a_weight_fails_check_naming_the_suite_and_the_file() {
    let dir = std::env::temp_dir().join(format!("cishard-ac01-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let weights = dir.join("weights.json");
    std::fs::write(
        &weights,
        r#"{"lib": 6, "suite_env_sandbox_01": 7, "suite_sandbox_02": 152}"#,
    )
    .unwrap();
    let o = partition(&["check"], Some(&weights));
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!o.status.success(), "check must fail when a weight is missing");
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("suite_sandbox_01"), "{err}");
    assert!(err.contains("scripts/ci-suite-weights.json"), "{err}");
}
