//! PRD-mcphost-msrv-locked-required-check AC2 -- every `cargo` command in the
//! `run:` steps of CI jobs `gate` (required) and `msrv` carries `--locked`, so
//! a stale `Cargo.lock` fails the job instead of being silently repaired in
//! the runner's checkout (leg 20: `cargo check --workspace` repaired #178's
//! lock and passed).

use crate::msrvlock;
use msrvlock as support;

const JOBS: [&str; 2] = ["gate", "msrv"];

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// `(job, 1-based line, trimmed command)` for every `cargo ...` command line
/// under a `run:` of the named jobs that does not contain `--locked`.
fn unlocked_cargo_lines(yaml: &str, jobs: &[&str]) -> Vec<(String, usize, String)> {
    let lines: Vec<&str> = yaml.lines().collect();
    let mut out = Vec::new();
    let mut job: Option<String> = None;
    let mut in_jobs = false;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let t = line.trim();
        if !t.is_empty() && !t.starts_with('#') && indent(line) == 0 {
            in_jobs = t == "jobs:";
            job = None;
        } else if in_jobs && indent(line) == 2 && t.ends_with(':') && !t.starts_with('#') {
            job = Some(t.trim_end_matches(':').to_string());
        }
        let watched = job.as_deref().is_some_and(|j| jobs.contains(&j));
        let run_key = t.strip_prefix("- ").unwrap_or(t);
        if watched && run_key.starts_with("run:") {
            let rest = run_key["run:".len()..].trim();
            let mut cmds: Vec<(usize, &str)> = Vec::new();
            if rest == "|" || rest == ">" || rest.starts_with("|-") || rest.starts_with("|+") {
                let base = indent(line);
                let mut j = i + 1;
                while j < lines.len()
                    && (lines[j].trim().is_empty() || indent(lines[j]) > base)
                {
                    cmds.push((j, lines[j].trim()));
                    j += 1;
                }
                i = j - 1;
            } else {
                cmds.push((i, rest));
            }
            for (idx, c) in cmds {
                if c.starts_with("cargo ") && !c.contains("--locked") {
                    out.push((job.clone().unwrap(), idx + 1, c.to_string()));
                }
            }
        }
        i += 1;
    }
    out
}

#[test]
fn msrvlock_ac02_gate_and_msrv_cargo_lines_all_carry_locked() {
    let yaml = std::fs::read_to_string(support::repo_root().join(".github/workflows/ci.yml"))
        .expect("read ci.yml");
    let bad = unlocked_cargo_lines(&yaml, &JOBS);
    assert!(
        bad.is_empty(),
        "cargo `run:` lines without --locked in ci.yml (job, line, command): {bad:?}"
    );
    // Guard against the parser silently matching nothing.
    let all = yaml.lines().filter(|l| l.trim().contains("cargo ")).count();
    assert!(all >= 6, "parser sanity: expected several cargo lines, saw {all}");
}

#[test]
fn msrvlock_ac02_fixture_with_one_unlocked_cargo_test_is_named() {
    let fixture = "\
jobs:
  gate:
    name: g
    steps:
      - name: a
        run: cargo check --locked --workspace
      - name: b
        run: |
          set -e
          cargo test --workspace
      - name: c
        run: cargo clippy --locked
  other:
    steps:
      - run: cargo build
  msrv:
    steps:
      - run: cargo check --locked --all-targets
";
    let bad = unlocked_cargo_lines(fixture, &JOBS);
    assert_eq!(
        bad,
        vec![("gate".to_string(), 10, "cargo test --workspace".to_string())],
        "exactly the unlocked `cargo test` line in `gate` must be named"
    );
}
