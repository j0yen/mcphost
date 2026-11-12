//! PRD-mcphost-outsch-ac01-coverage-diagnosable AC2 -- given `target/outsch`
//! empty, partial and full, the coverage and branch-budget tests give the same
//! verdict and an invalid captured row fails the latter; 30 of 50 covered with
//! the 20 in `GAPS.txt` is `Err`; the committed fixtures cover
//! `ceil(0.8 x registered)` tools, each `GAPS.txt` line has a reason, each
//! `host.queue.*` tool has a captured fixture, and only the branch-budget test
//! and this file call `outsch_rows(`.

use std::path::{Path, PathBuf};

use mcphost::output_schemas::registered_tools;
use serde_json::json;

use crate::common;

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("outcov-ac02-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Copies `names` (file names under the committed corpus) into `dir`.
fn copy_rows(dir: &Path, names: &[String]) {
    for name in names {
        std::fs::copy(common::outsch_fixture_dir().join(name), dir.join(name)).unwrap_or_else(|e| panic!("copy {name}: {e}"));
    }
}

fn committed_jsonl_names() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(common::outsch_fixture_dir())
        .expect("fixtures")
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.ends_with(".jsonl"))
        .collect();
    names.sort();
    names
}

/// The coverage verdict as the gate computes it: committed rows and the register.
fn coverage_verdict() -> Result<(), String> {
    let fixtures = common::outsch_fixture_dir();
    common::coverage_report(&registered_tools(), &common::outsch_committed_rows(&fixtures), &common::outsch_gaps(&fixtures))
}

#[test]
fn outcov_ac02_the_corpus_covers_at_least_eighty_percent_of_registered_tools() {
    if let Err(e) = coverage_verdict() {
        panic!("{e}");
    }
    let tools = registered_tools();
    let rows = common::outsch_committed_rows(&common::outsch_fixture_dir());
    let covered = tools.iter().filter(|t| rows.get(**t).is_some_and(|r| !r.is_empty())).count();
    let needed = (tools.len() * 8).div_ceil(10);
    assert!(covered >= needed, "committed fixtures cover {covered} of {}, need ceil(0.8 x registered) = {needed}", tools.len());
}

#[test]
fn outcov_ac02_every_gaps_line_has_a_reason_and_a_registered_uncovered_tool() {
    let fixtures = common::outsch_fixture_dir();
    let gaps = common::outsch_gaps(&fixtures);
    let tools = registered_tools();
    let rows = common::outsch_committed_rows(&fixtures);
    for (tool, reason) in &gaps {
        assert!(!reason.trim().is_empty(), "GAPS.txt: {tool} has no reason");
        assert!(tools.contains(&tool.as_str()), "GAPS.txt: {tool} is not a registered tool");
        assert!(rows.get(tool).is_none_or(Vec::is_empty), "GAPS.txt: {tool} has a fixture, delete its line");
    }
}

#[test]
fn outcov_ac02_every_queue_tool_has_a_captured_fixture() {
    let rows = common::outsch_committed_rows(&common::outsch_fixture_dir());
    let queue_tools: Vec<&str> = registered_tools().into_iter().filter(|t| t.starts_with("host.queue.")).collect();
    assert_eq!(queue_tools.len(), 8, "the eight host.queue.* verbs are registered: {queue_tools:?}");
    for tool in queue_tools {
        assert!(rows.get(tool).is_some_and(|r| !r.is_empty()), "{tool} has no committed fixture");
    }
}

#[test]
fn outcov_ac02_empty_partial_and_full_capture_dirs_give_one_verdict() {
    let names = committed_jsonl_names();
    let fixtures = common::outsch_fixture_dir();
    let empty = scratch("empty");
    let partial = scratch("partial");
    copy_rows(&partial, &names[..names.len() / 3]);
    let full = scratch("full");
    copy_rows(&full, &names);

    let coverage = coverage_verdict();
    let mut branch = Vec::new();
    for dir in [&empty, &partial, &full] {
        let rows = common::outsch_rows(&fixtures, dir);
        // The coverage test never reads the capture dir; the rows it would see are the committed ones.
        assert_eq!(
            common::coverage_report(&registered_tools(), &common::outsch_committed_rows(&fixtures), &common::outsch_gaps(&fixtures)),
            coverage
        );
        branch.push(common::branch_budget_verdict(&rows));
    }
    assert_eq!(branch[0], branch[1], "empty vs partial capture");
    assert_eq!(branch[1], branch[2], "partial vs full capture");
    assert_eq!(branch[0], Ok(()), "the committed corpus is within the branch budget");
    assert_eq!(coverage, Ok(()), "the committed corpus meets the coverage rule");
    for dir in [empty, partial, full] {
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[test]
fn outcov_ac02_an_invalid_captured_row_fails_the_branch_budget_test_only() {
    let fixtures = common::outsch_fixture_dir();
    let capture = scratch("invalid");
    std::fs::write(capture.join("host.whoami.jsonl"), format!("{}\n", json!({"not": "a whoami"}))).unwrap();
    let verdict = common::branch_budget_verdict(&common::outsch_rows(&fixtures, &capture));
    let err = verdict.expect_err("a row that fails its schema fails the branch-budget test");
    assert!(err.contains("host.whoami"), "{err}");
    assert_eq!(coverage_verdict(), Ok(()), "the coverage verdict ignores the capture dir");
    let _ = std::fs::remove_dir_all(capture);
}

#[test]
fn outcov_ac02_thirty_of_fifty_covered_with_the_twenty_in_gaps_txt_is_err() {
    let fixtures = scratch("fifty");
    let names: Vec<String> = (0..50).map(|i| format!("t.{i:02}")).collect();
    for name in &names[..30] {
        std::fs::write(fixtures.join(format!("{name}.jsonl")), "{\"ok\":true}\n").unwrap();
    }
    let register: String = names[30..].iter().map(|n| format!("{n} no capture path\n")).collect();
    std::fs::write(fixtures.join("GAPS.txt"), register).unwrap();
    let tools: Vec<&str> = names.iter().map(String::as_str).collect();
    let err = common::coverage_report(&tools, &common::outsch_committed_rows(&fixtures), &common::outsch_gaps(&fixtures))
        .expect_err("30/50 is 60 %");
    assert!(err.starts_with("30/50"), "{err}");
    let _ = std::fs::remove_dir_all(fixtures);
}

#[test]
fn outcov_ac02_only_the_branch_budget_test_and_this_file_call_outsch_rows() {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("read tests dir").flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut files = Vec::new();
    walk(&tests, &mut files);
    let needle = ["outsch_rows", "("].concat();
    let mut callers: Vec<String> = Vec::new();
    for path in files {
        let rel = path.strip_prefix(&tests).unwrap().to_string_lossy().into_owned();
        if rel == "common/mod.rs" {
            continue; // the definition
        }
        if std::fs::read_to_string(&path).unwrap_or_default().contains(&needle) {
            callers.push(rel);
        }
    }
    callers.sort();
    assert!(
        callers.iter().all(|c| c == "outsch_ac01_oneof_budget_falsifier.rs" || c.starts_with("outcov_ac02_")),
        "only the branch-budget test and outcov_ac02_*.rs call outsch_rows: {callers:?}"
    );
}
