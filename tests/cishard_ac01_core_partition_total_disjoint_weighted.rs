//! contract: core suites are split into generated weight-balanced shards
//! PRD-mcphost-ci-core-suites-sharded AC1 -- every core suite binary appears in
//! exactly one `core-shard <n> <CORE_SHARDS>` output, each has a weight in
//! scripts/ci-suite-weights.json, and a suite missing a weight fails
//! `ci-test-partition.sh check` naming it.

use std::path::PathBuf;
use std::process::{Command, Output};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn partition(args: &[&str], weights: Option<&PathBuf>, core_shards: Option<&str>) -> Output {
    let mut cmd = Command::new(root().join("scripts/ci-test-partition.sh"));
    cmd.args(args).current_dir(root());
    if let Some(w) = weights {
        cmd.env("CI_SUITE_WEIGHTS_FILE", w);
    }
    if let Some(n) = core_shards {
        cmd.env("CORE_SHARDS", n);
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

fn core_shard_count() -> usize {
    stdout(&partition(&["shards", "core"], None, None))
        .trim()
        .parse()
        .expect("`shards core` prints the core shard count")
}

fn weights() -> serde_json::Map<String, serde_json::Value> {
    let text = std::fs::read_to_string(root().join("scripts/ci-suite-weights.json")).expect("weights file");
    serde_json::from_str::<serde_json::Value>(&text)
        .expect("weights file is JSON")
        .as_object()
        .expect("weights file is an object")
        .clone()
}

#[test]
fn the_core_shards_are_total_and_disjoint_over_the_core_suite_list() {
    let total = core_shard_count();
    assert!(total >= 2, "a single core shard is the unsharded job again: {total}");
    let shards: Vec<String> = (1..=total)
        .map(|n| stdout(&partition(&["core-shard", &n.to_string(), &total.to_string()], None, None)))
        .collect();

    let mut union: Vec<String> = shards.iter().flat_map(|s| suites_of(s)).collect();
    union.sort();
    let mut deduped = union.clone();
    deduped.dedup();
    assert_eq!(union, deduped, "a core suite appears in more than one shard");

    let mut listed: Vec<String> = stdout(&partition(&["list", "core"], None, None))
        .lines()
        .map(str::to_string)
        .collect();
    listed.sort();
    assert_eq!(union, listed, "the core shards must be total over `list core`");

    for (i, s) in shards.iter().enumerate() {
        assert!(!suites_of(s).is_empty(), "core shard {}/{total} is empty", i + 1);
    }
}

#[test]
fn bins_run_once_on_shard_one() {
    let total = core_shard_count();
    for n in 1..=total {
        let flags = stdout(&partition(&["core-shard", &n.to_string(), &total.to_string()], None, None));
        assert_eq!(flags.contains("--bins"), n == 1, "--bins placement on shard {n}: {flags}");
    }
}

#[test]
fn every_core_suite_has_a_recorded_weight() {
    let w = weights();
    for s in stdout(&partition(&["list", "core"], None, None)).lines() {
        let v = w.get(s).unwrap_or_else(|| panic!("core suite {s} has no weight in scripts/ci-suite-weights.json"));
        assert!(v.as_f64().is_some_and(|x| x >= 0.0), "weight of {s} is not a number: {v}");
    }
}

#[test]
fn the_heaviest_suites_do_not_share_a_shard() {
    // Placement is by cost: the two heaviest core suites must land apart, or
    // the split would not shorten the longest shard at all.
    let total = core_shard_count();
    let w = weights();
    let mut by_weight: Vec<(f64, String)> = stdout(&partition(&["list", "core"], None, None))
        .lines()
        .map(|s| (w[s].as_f64().unwrap(), s.to_string()))
        .collect();
    by_weight.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then(a.1.cmp(&b.1)));
    let shard_of = |suite: &str| -> usize {
        (1..=total)
            .find(|n| {
                suites_of(&stdout(&partition(&["core-shard", &n.to_string(), &total.to_string()], None, None)))
                    .contains(&suite.to_string())
            })
            .expect("suite is in a shard")
    };
    assert_ne!(
        shard_of(&by_weight[0].1),
        shard_of(&by_weight[1].1),
        "{} and {} are the two heaviest core suites and must not share a shard",
        by_weight[0].1,
        by_weight[1].1
    );
}

#[test]
fn a_core_suite_without_a_weight_fails_check_naming_the_suite_and_the_file() {
    let dropped = stdout(&partition(&["list", "core"], None, None))
        .lines()
        .next()
        .expect("at least one core suite")
        .to_string();
    let mut w = weights();
    w.remove(&dropped);
    let dir = std::env::temp_dir().join(format!("cishard-core-ac01-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("weights.json");
    std::fs::write(&file, serde_json::to_string(&w).unwrap()).unwrap();
    let o = partition(&["check"], Some(&file), None);
    let o_shard = partition(&["core-shard", "1", "3"], Some(&file), None);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!o.status.success(), "check must fail when a core weight is missing");
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains(&dropped), "check must name {dropped}: {err}");
    assert!(err.contains("scripts/ci-suite-weights.json"), "{err}");
    assert!(!o_shard.status.success(), "core-shard must refuse to guess a missing weight");
}

#[test]
fn check_fails_naming_an_empty_shard_when_the_count_outgrows_the_suites() {
    let n_suites = stdout(&partition(&["list", "core"], None, None)).lines().count();
    let too_many = (n_suites + 1).to_string();
    let o = partition(&["check"], None, Some(&too_many));
    assert!(!o.status.success(), "{n_suites} suites cannot fill {too_many} shards");
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains(&format!("core shard {too_many}/{too_many} is empty")) || err.contains("is empty"), "{err}");
}
