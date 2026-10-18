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
//!   2. that budget is proven against the pre-conversion source captured in
//!      `tests/fixtures/tlint_pre_conversion_budgets.json` (from
//!      `git show 1f7bfb4:<file>`; CI clones are shallow so git is not read at
//!      test time). The `#[ignore]` `regenerate_pre_conversion_fixture`
//!      rebuilds it from a full clone.

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
    Site {
        file: "xlinks_ac03_head_405_falls_back_and_timeout_fails.rs",
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

const FIXTURE: &str = "tests/fixtures/tlint_pre_conversion_budgets.json";

/// Pre-conversion evidence for `site`: the trimmed source lines of
/// `PRE_CONVERSION:tests/<file>` that hold each proving substring. Reads git.
fn evidence_from_git(root: &std::path::Path, site: &Site) -> Vec<String> {
    let out = Command::new("git")
        .current_dir(root)
        .args(["show", &format!("{PRE_CONVERSION}:tests/{}", site.file)])
        .output()
        .expect("run git show");
    assert!(out.status.success(), "git show {}: {}", site.file, String::from_utf8_lossy(&out.stderr));
    let old = String::from_utf8_lossy(&out.stdout).into_owned();
    site.original
        .iter()
        .map(|needle| {
            old.lines()
                .find(|l| l.contains(needle))
                .unwrap_or_else(|| panic!("{}: pre-conversion source lacks `{needle}`", site.file))
                .trim()
                .to_string()
        })
        .collect()
}

/// Rebuild the committed fixture from git history. Needs a full clone:
/// `cargo test --test <the suite that includes this file, see `suite_holding`>
/// regenerate_pre_conversion_fixture -- --ignored` (or this file's own test target).
#[test]
#[ignore]
fn regenerate_pre_conversion_fixture() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut files = serde_json::Map::new();
    let mut sorted: Vec<&Site> = CONVERTED.iter().collect();
    sorted.sort_by_key(|s| s.file);
    for site in sorted {
        let lines = evidence_from_git(&root, site);
        files.insert(site.file.to_string(), serde_json::json!({ "budgets_ms": site.budgets_ms, "evidence_lines": lines }));
    }
    let doc = serde_json::json!({ "source_commit": PRE_CONVERSION, "files": files });
    fs::write(root.join(FIXTURE), serde_json::to_string_pretty(&doc).unwrap() + "\n").expect("write fixture");
}

#[test]
fn pre_conversion_source_holds_the_budget_each_helper_literal_claims() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let raw = fs::read_to_string(root.join(FIXTURE)).unwrap_or_else(|e| panic!("read {FIXTURE}: {e}"));
    let doc: serde_json::Value = serde_json::from_str(&raw).expect("fixture is JSON");
    assert_eq!(doc["source_commit"], PRE_CONVERSION, "fixture was captured from a different commit");
    let files = doc["files"].as_object().expect("fixture `files` object");
    let mut problems = Vec::new();
    for site in CONVERTED {
        let Some(entry) = files.get(site.file) else {
            problems.push(format!("{}: missing from fixture", site.file));
            continue;
        };
        let budgets: Vec<u64> = entry["budgets_ms"].as_array().map(|a| a.iter().filter_map(|v| v.as_u64()).collect()).unwrap_or_default();
        if budgets != site.budgets_ms {
            problems.push(format!("{}: fixture budgets {budgets:?}, site claims {:?}", site.file, site.budgets_ms));
        }
        let lines: Vec<&str> = entry["evidence_lines"].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).collect()).unwrap_or_default();
        for needle in site.original {
            if !lines.iter().any(|l| l.contains(needle)) {
                problems.push(format!("{}: pre-conversion source lacks `{needle}`", site.file));
            }
        }
    }
    for name in files.keys() {
        if !CONVERTED.iter().any(|s| s.file == name) {
            problems.push(format!("{name}: in fixture but not a converted site"));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// The test binary (suite) that compiles `file` in as a module, read from the
/// generated `tests/suite_*.rs` files (scripts/gen-test-suites.sh) so a re-cut
/// of the suite layout can never leave this test filtering an empty binary.
fn suite_holding(root: &std::path::Path, file: &str) -> String {
    let needle = format!("#[path = \"{file}\"]");
    let mut found = Vec::new();
    for entry in fs::read_dir(root.join("tests")).expect("read tests/") {
        let path = entry.expect("dir entry").path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        if name.starts_with("suite_") && name.ends_with(".rs") && fs::read_to_string(&path).unwrap_or_default().contains(&needle) {
            found.push(name.trim_end_matches(".rs").to_string());
        }
    }
    assert_eq!(found.len(), 1, "{file}: expected exactly one generated suite to include it, found {found:?}");
    found.remove(0)
}

/// Run every converted site's tests (`cargo test --test <suite> <module>::`)
/// once per env setting and require them all to pass.
#[test]
fn every_converted_site_passes_with_and_without_perf_skip() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let cargo = option_env!("CARGO").unwrap_or("cargo");
    let suite_of: Vec<(&Site, String)> = CONVERTED.iter().map(|s| (s, suite_holding(&root, s.file))).collect();
    let mut suites: Vec<&str> = suite_of.iter().map(|(_, suite)| suite.as_str()).collect();
    suites.sort();
    suites.dedup();
    let mut problems = Vec::new();
    for skip in [false, true] {
        for suite in &suites {
            // The strict (unset) run measures real wall time while the outer
            // suite may be saturating the host, so a miss is retried
            // (serially) up to twice before it counts; a genuinely broken budget fails all three.
            let mut attempt = 0;
            let (out, text, ran) = loop {
                attempt += 1;
                let mut cmd = Command::new(cargo);
                cmd.current_dir(&root).args(["test", "--test", suite, "--features", "test-support", "--"]);
                for (site, _) in suite_of.iter().filter(|(_, s)| s == suite) {
                    cmd.arg(format!("{}::", site.file.trim_end_matches(".rs")));
                }
                cmd.arg("--test-threads=1");
                if skip {
                    cmd.env("MCPHOST_PERF_SKIP", "1");
                } else {
                    cmd.env_remove("MCPHOST_PERF_SKIP");
                }
                let out = cmd.output().expect("spawn nested cargo test");
                let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
                let ran = text.lines().any(|l| l.starts_with("test result: ok. ") && !l.contains(" 0 passed"));
                if (out.status.success() && ran) || attempt >= 3 {
                    break (out, text, ran);
                }
            };
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
