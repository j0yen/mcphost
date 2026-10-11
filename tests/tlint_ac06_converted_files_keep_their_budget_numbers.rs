//! PRD-mcphost-timing-assertion-lint
//! AC6 (P0) — Given the 4 former `ALLOW` files and the budget sites named in
//! Technical considerations, When `cargo test` runs each converted file with
//! and without `MCPHOST_PERF_SKIP=1`, Then every test passes and no budget
//! number changed.
//!
//! `every_converted_site_passes_with_and_without_perf_skip` runs each converted
//! site's tests through a nested `cargo test` once with and once without
//! `MCPHOST_PERF_SKIP=1`. The other two tests pin "no budget number changed":
//!   1. every `deadline_budget!(N` literal PARSED out of each converted file
//!      must equal exactly the pre-conversion budget in ms;
//!   2. that budget is proven against the pre-conversion source
//!      (`git show 1f7bfb4:<file>`), which must still hold the original
//!      wall-clock expression; an unreachable commit FAILS the test.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Last commit before AC5 converted the files to the timing helpers.
const PRE_CONVERSION: &str = "1f7bfb4";

struct Site {
    file: &'static str,
    /// Test binary (suite) that compiles this file in as a module.
    suite: &'static str,
    /// Every `deadline_budget!` literal the converted file must hold, in order
    /// (the pre-conversion budget in ms).
    budgets_ms: &'static [u64],
    /// Substring the pre-conversion source must contain, proving where each
    /// budget number came from.
    original: &'static [&'static str],
}

const CONVERTED: &[Site] = &[
    // the 4 former ALLOW files
    Site {
        file: "checkcompat_race_ac01_previous_up_within_1s.rs",
        suite: "suite_core_07",
        budgets_ms: &[1000],
        original: &["as_secs_f64() < 1.0"],
    },
    Site {
        file: "python_ac01_no_deps_cpu_memory.rs",
        suite: "suite_sandbox_02",
        budgets_ms: &[5000],
        original: &["started.elapsed() < Duration::from_secs(5)"],
    },
    Site {
        file: "ac15_call_timeout.rs",
        suite: "suite_core_07",
        budgets_ms: &[5000],
        original: &["started.elapsed() < Duration::from_secs(5)"],
    },
    Site {
        file: "tablemodel_ac06_large_table_samples_10000_under_500ms.rs",
        suite: "suite_core_08",
        budgets_ms: &[500],
        original: &["elapsed.as_millis() < 500"],
    },
    // budget sites named in Technical considerations
    Site {
        file: "drift_ac09_fifty_queries_rerun_under_5s_one_call.rs",
        suite: "suite_core_07",
        budgets_ms: &[5000],
        original: &["elapsed < Duration::from_secs(5)"],
    },
    Site {
        file: "hooks_ac1_hmac_sha256_valid_signature_runs.rs",
        suite: "suite_core_07",
        budgets_ms: &[500],
        original: &["elapsed < Duration::from_millis(500)"],
    },
    Site {
        file: "oauthconf_ac01_gate_verdict_table_matches_gold.rs",
        suite: "suite_core_07",
        budgets_ms: &[20_000],
        original: &["elapsed.as_secs() < 20"],
    },
    Site {
        file: "wake_ac6_msg_wait.rs",
        suite: "suite_core_08",
        budgets_ms: &[3000, 4000],
        original: &["elapsed < Duration::from_secs(3)", "elapsed <= Duration::from_secs(4)"],
    },
    Site {
        file: "xlinks_ac07_ci_docs_job_fails_on_a_404_and_stays_fast.rs",
        suite: "suite_core_08",
        budgets_ms: &[60_000],
        original: &["took < std::time::Duration::from_secs(60)"],
    },
    Site {
        file: "xlinks_ac03_head_405_falls_back_and_timeout_fails.rs",
        suite: "suite_core_08",
        budgets_ms: &[10_000],
        original: &["xlinks::run_checker(&[&file]);", "elapsed.as_millis() >= 1900"],
    },
];

/// Every integer literal following `deadline_budget!(` in `src`, ignoring
/// `_` separators.
fn parse_budgets(src: &str) -> Vec<u64> {
    src.match_indices("deadline_budget!(")
        .filter_map(|(i, m)| {
            let rest = &src[i + m.len()..];
            let end = rest.find(|c: char| !(c.is_ascii_digit() || c == '_'))?;
            rest[..end].replace('_', "").parse().ok()
        })
        .collect()
}

#[test]
fn every_converted_file_spells_its_original_budget_through_a_helper() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut problems = Vec::new();
    for site in CONVERTED {
        let src = fs::read_to_string(root.join("tests").join(site.file))
            .unwrap_or_else(|e| panic!("read {}: {e}", site.file));
        // The macro definition lives in common/mod.rs, so every hit here is a call.
        let got = parse_budgets(&src);
        if got != site.budgets_ms {
            problems.push(format!("{}: budgets {got:?}, pre-conversion {:?}", site.file, site.budgets_ms));
        }
    }
    // sqlpass was already on the overhead helper before AC5; it must stay on it.
    let sql = fs::read_to_string(root.join("tests/sqlpass_ac04_group_by_logs_row_count_and_duration.rs"))
        .expect("read sqlpass_ac04");
    if !sql.contains("perf_overhead_budget!(\n        5,") {
        problems.push("sqlpass_ac04: perf_overhead_budget!(5, ..) gone or its 5 ms budget changed".into());
    }
    assert!(problems.is_empty(), "budget sites changed or unwrapped:\n{}", problems.join("\n"));
}

#[test]
fn pre_conversion_source_holds_the_budget_each_helper_literal_claims() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let probe = Command::new("git")
        .current_dir(&root)
        .args(["cat-file", "-e", &format!("{PRE_CONVERSION}^{{commit}}")])
        .output();
    match probe {
        Ok(o) if o.status.success() => {}
        _ => panic!("pre-conversion commit {PRE_CONVERSION} is not reachable; fetch full history, the cross-check cannot be skipped"),
    }
    let mut problems = Vec::new();
    for site in CONVERTED {
        let out = Command::new("git")
            .current_dir(&root)
            .args(["show", &format!("{PRE_CONVERSION}:tests/{}", site.file)])
            .output()
            .expect("run git show");
        assert!(out.status.success(), "git show {}: {}", site.file, String::from_utf8_lossy(&out.stderr));
        let old = String::from_utf8_lossy(&out.stdout);
        for needle in site.original {
            if !old.contains(needle) {
                problems.push(format!("{}: pre-conversion source lacks `{needle}`", site.file));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// Run every converted site's tests (`cargo test --test <suite> <module>::`)
/// once per env setting and require them all to pass.
#[test]
fn every_converted_site_passes_with_and_without_perf_skip() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let cargo = option_env!("CARGO").unwrap_or("cargo");
    let mut suites: Vec<&str> = CONVERTED.iter().map(|s| s.suite).collect();
    suites.sort();
    suites.dedup();
    let mut problems = Vec::new();
    for skip in [false, true] {
        for suite in &suites {
            let mut cmd = Command::new(cargo);
            cmd.current_dir(&root).args(["test", "--test", suite, "--features", "test-support", "--"]);
            for site in CONVERTED.iter().filter(|s| s.suite == *suite) {
                cmd.arg(format!("{}::", site.file.trim_end_matches(".rs")));
            }
            if skip {
                cmd.env("MCPHOST_PERF_SKIP", "1");
            } else {
                cmd.env_remove("MCPHOST_PERF_SKIP");
            }
            let out = cmd.output().expect("spawn nested cargo test");
            let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            let ran = text.lines().any(|l| l.starts_with("test result: ok. ") && !l.contains(" 0 passed"));
            if !out.status.success() || !ran {
                let tail: Vec<&str> = text.lines().rev().take(25).collect();
                problems.push(format!(
                    "{suite} (MCPHOST_PERF_SKIP={}): status {:?}, ran>0={ran}\n{}",
                    if skip { "1" } else { "unset" },
                    out.status.code(),
                    tail.into_iter().rev().collect::<Vec<_>>().join("\n")
                ));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n---\n"));
}
