//! PRD-mcphost-ci-sandbox-shards-balanced AC2 -- the sandbox shard count is
//! defined once (`SANDBOX_SHARDS` in scripts/ci-test-partition.sh); `shards`
//! prints it, ci.yml derives its matrix and job names from that command,
//! gen-test-suites.sh reads it, and no literal count reappears in either file.

use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

fn shards_cmd(env: Option<&str>) -> String {
    let mut cmd = Command::new(root().join("scripts/ci-test-partition.sh"));
    cmd.arg("shards").current_dir(root()).env_remove("SANDBOX_SHARDS");
    if let Some(v) = env {
        cmd.env("SANDBOX_SHARDS", v);
    }
    let o = cmd.output().expect("run ci-test-partition.sh shards");
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

/// The literal-count shapes that must never appear in the consumers.
fn literal_counts(text: &str) -> Vec<String> {
    let mut hits = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let digit_after = |s: &str| {
            line.find(s)
                .map(|p| line[p + s.len()..].trim_start().starts_with(|c: char| c.is_ascii_digit()))
                .unwrap_or(false)
        };
        let mut bad = digit_after("SANDBOX_SHARDS=")
            || digit_after("SANDBOX_SHARDS =")
            || line.contains("shard: [")
            || line.contains("-way");
        // `shard N/<digit>` / `shard ${{ matrix.shard }}/<digit>`
        let mut from = 0;
        while let Some(p) = line[from..].find("shard") {
            let rest = &line[from + p + 5..];
            if let Some(slash) = rest.find('/')
                && slash <= 24
                && rest[slash + 1..].starts_with(|c: char| c.is_ascii_digit())
            {
                bad = true;
            }
            from += p + 5;
        }
        if bad {
            hits.push(format!("{}: {}", i + 1, line.trim()));
        }
    }
    hits
}

#[test]
fn shards_prints_the_one_defined_count() {
    let script = read("scripts/ci-test-partition.sh");
    let defs: Vec<&str> = script
        .lines()
        .filter(|l| l.trim_start().starts_with("SANDBOX_SHARDS="))
        .collect();
    assert_eq!(defs.len(), 1, "exactly one SANDBOX_SHARDS definition: {defs:?}");
    assert_eq!(shards_cmd(None), "3");
    assert_eq!(shards_cmd(Some("4")), "4", "the printed count comes from the definition");
}

#[test]
fn ci_yml_derives_matrix_and_job_names_from_the_command() {
    let ci = read(".github/workflows/ci.yml");
    assert!(ci.contains("./scripts/ci-test-partition.sh shards"), "plan job must call `shards`");
    assert!(
        ci.contains("matrix: ${{ fromJson(needs.plan.outputs.matrix) }}"),
        "sandbox matrix must come from the plan job"
    );
    assert!(
        ci.contains("shard ${{ matrix.shard }}/${{ needs.plan.outputs.shards }})"),
        "job name total must come from the plan job"
    );
}

#[test]
fn gen_test_suites_reads_the_command() {
    let gen_script = read("scripts/gen-test-suites.sh");
    assert!(
        gen_script.contains(r#"[CLASSIFY_SCRIPT, "shards"]"#),
        "gen-test-suites.sh must read `ci-test-partition.sh shards`"
    );
}

#[test]
fn no_literal_shard_count_in_ci_yml_or_gen_test_suites() {
    for rel in [".github/workflows/ci.yml", "scripts/gen-test-suites.sh"] {
        let hits = literal_counts(&read(rel));
        assert!(hits.is_empty(), "{rel} carries a literal shard count:\n{}", hits.join("\n"));
    }
}

#[test]
fn the_drift_scan_catches_a_reintroduced_literal() {
    assert!(!literal_counts("matrix:\n  shard: [1, 2]").is_empty());
    assert!(!literal_counts("name: x (shard ${{ matrix.shard }}/2)").is_empty());
    assert!(!literal_counts("# SANDBOX_SHARDS=2 matrix").is_empty());
    assert!(literal_counts("name: x (shard ${{ matrix.shard }}/${{ needs.plan.outputs.shards }})").is_empty());
}
