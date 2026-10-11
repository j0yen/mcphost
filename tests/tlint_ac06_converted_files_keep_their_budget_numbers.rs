//! PRD-mcphost-timing-assertion-lint
//! AC6 (P0) — Given the 4 former `ALLOW` files and the budget sites named in
//! Technical considerations, When `cargo test` runs each converted file with
//! and without `MCPHOST_PERF_SKIP=1`, Then every test passes and no budget
//! number changed.
//!
//! The with/without-skip runs are the suite itself (the gate runs it both
//! ways, and `deadline_budget!` behaviour under skip is pinned by AC4). This
//! file pins the second half "no budget number changed" two ways:
//!   1. every `deadline_budget!(N` literal PARSED out of each converted file
//!      must equal exactly the pre-conversion budget in ms (an added, dropped,
//!      or loosened budget fails);
//!   2. that pre-conversion budget is itself proven against the pre-conversion
//!      source (`git show 1f7bfb4:<file>`, the commit before AC5 converted the
//!      files) which must still contain the original wall-clock expression.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Last commit before AC5 converted the files to the timing helpers.
const PRE_CONVERSION: &str = "1f7bfb4";

struct Site {
    file: &'static str,
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
        budgets_ms: &[1000],
        original: &["as_secs_f64() < 1.0"],
    },
    Site {
        file: "python_ac01_no_deps_cpu_memory.rs",
        budgets_ms: &[5000],
        original: &["started.elapsed() < Duration::from_secs(5)"],
    },
    Site {
        file: "ac15_call_timeout.rs",
        budgets_ms: &[5000],
        original: &["started.elapsed() < Duration::from_secs(5)"],
    },
    Site {
        file: "tablemodel_ac06_large_table_samples_10000_under_500ms.rs",
        budgets_ms: &[500],
        original: &["elapsed.as_millis() < 500"],
    },
    // budget sites named in Technical considerations
    Site {
        file: "drift_ac09_fifty_queries_rerun_under_5s_one_call.rs",
        budgets_ms: &[5000],
        original: &["elapsed < Duration::from_secs(5)"],
    },
    Site {
        file: "hooks_ac1_hmac_sha256_valid_signature_runs.rs",
        budgets_ms: &[500],
        original: &["elapsed < Duration::from_millis(500)"],
    },
    Site {
        file: "oauthconf_ac01_gate_verdict_table_matches_gold.rs",
        budgets_ms: &[20_000],
        original: &["elapsed.as_secs() < 20"],
    },
    Site {
        file: "wake_ac6_msg_wait.rs",
        budgets_ms: &[3000, 4000],
        original: &["elapsed < Duration::from_secs(3)", "elapsed <= Duration::from_secs(4)"],
    },
    Site {
        file: "xlinks_ac07_ci_docs_job_fails_on_a_404_and_stays_fast.rs",
        budgets_ms: &[60_000],
        original: &["took < std::time::Duration::from_secs(60)"],
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
    if !sql.contains("perf_overhead_budget!(") {
        problems.push("sqlpass_ac04: perf_overhead_budget! gone".into());
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
        // Shallow clone / history rewritten: the literal pin above still holds.
        _ => {
            eprintln!("pre-conversion commit {PRE_CONVERSION} not reachable; skipping git cross-check");
            return;
        }
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
