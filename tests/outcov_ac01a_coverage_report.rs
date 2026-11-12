//! PRD-mcphost-outsch-ac01-coverage-diagnosable AC1 (coverage half) -- given
//! `coverage_report` over 10 registered tools with rows for 7, it is `Err`
//! naming the 3 uncovered tools and `7/10`; with rows for 8 and the other 2 in
//! `GAPS.txt` it is `Ok`. The register is exact: a stale, unregistered,
//! reason-less or doubled line is an `Err`, and `GAPS.txt` beside the fixtures
//! is never read as a row.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::common;

const TOOLS: [&str; 10] = ["t.a", "t.b", "t.c", "t.d", "t.e", "t.f", "t.g", "t.h", "t.i", "t.j"];

fn rows_for(tools: &[&str]) -> BTreeMap<String, Vec<Value>> {
    tools.iter().map(|t| (t.to_string(), vec![json!({"ok": true})])).collect()
}

fn gaps(text: &str) -> Vec<(String, String)> {
    common::outsch_gaps_parse(text)
}

#[test]
fn outcov_ac01a_seven_of_ten_is_err_naming_count_and_the_three_missing() {
    let err = common::coverage_report(&TOOLS, &rows_for(&TOOLS[..7]), &[]).expect_err("7/10 is under 80 %");
    assert!(err.starts_with("7/10"), "the counts lead the text so a truncated excerpt keeps them: {err}");
    for missing in ["t.h", "t.i", "t.j"] {
        assert!(err.contains(missing), "{missing} not named: {err}");
    }
    for covered in ["t.a", "t.g"] {
        assert!(!err.contains(&format!("\"{covered}\"")), "{covered} is covered and must not be listed: {err}");
    }
    let (h, i, j) = (err.find("t.h").unwrap(), err.find("t.i").unwrap(), err.find("t.j").unwrap());
    assert!(h < i && i < j, "uncovered tools are listed sorted: {err}");
}

#[test]
fn outcov_ac01a_eight_of_ten_with_the_other_two_registered_is_ok() {
    let register = gaps("t.i no capture path for this verb\nt.j needs a live upstream\n");
    common::coverage_report(&TOOLS, &rows_for(&TOOLS[..8]), &register).expect("8/10 with both gaps registered");
}

#[test]
fn outcov_ac01a_eight_of_ten_with_a_gap_missing_from_the_register_is_err() {
    let err = common::coverage_report(&TOOLS, &rows_for(&TOOLS[..8]), &gaps("t.i reason\n")).expect_err("t.j is in no register");
    assert!(err.starts_with("8/10"), "{err}");
    assert!(err.contains("t.j"), "{err}");
}

#[test]
fn outcov_ac01a_the_register_never_counts_toward_coverage() {
    let register = gaps("t.h a\nt.i b\nt.j c\n");
    let err = common::coverage_report(&TOOLS, &rows_for(&TOOLS[..7]), &register).expect_err("registered gaps leave 7/10");
    assert!(err.starts_with("7/10"), "{err}");
    assert!(err.contains("80 %"), "{err}");
}

#[test]
fn outcov_ac01a_thirty_of_fifty_covered_with_twenty_registered_is_err() {
    let names: Vec<String> = (0..50).map(|i| format!("t.{i:02}")).collect();
    let tools: Vec<&str> = names.iter().map(String::as_str).collect();
    let register: Vec<(String, String)> = tools[30..].iter().map(|t| (t.to_string(), "reason".to_string())).collect();
    let err = common::coverage_report(&tools, &rows_for(&tools[..30]), &register).expect_err("30/50 is 60 %");
    assert!(err.starts_with("30/50"), "{err}");
}

#[test]
fn outcov_ac01a_a_stale_unregistered_reasonless_or_doubled_line_is_err() {
    let covered = rows_for(&TOOLS[..8]);
    let stale = common::coverage_report(&TOOLS, &covered, &gaps("t.a reason\nt.i r\nt.j r\n")).expect_err("t.a has rows");
    assert!(stale.contains("now have a fixture") && stale.contains("t.a"), "{stale}");
    let unknown = common::coverage_report(&TOOLS, &covered, &gaps("t.i r\nt.j r\nnot.registered r\n")).expect_err("unknown tool");
    assert!(unknown.contains("not registered") && unknown.contains("not.registered"), "{unknown}");
    let bare = common::coverage_report(&TOOLS, &covered, &gaps("t.i\nt.j r\n")).expect_err("t.i has no reason");
    assert!(bare.contains("without a reason") && bare.contains("t.i"), "{bare}");
    let twice = common::coverage_report(&TOOLS, &covered, &gaps("t.i r\nt.i again\nt.j r\n")).expect_err("t.i twice");
    assert!(twice.contains("twice") && twice.contains("t.i"), "{twice}");
}

#[test]
fn outcov_ac01a_gaps_parse_takes_the_first_token_as_the_tool() {
    let parsed = gaps("# header\n\nhost.a  needs a live upstream \nhost.b\n");
    assert_eq!(
        parsed,
        vec![("host.a".to_string(), "needs a live upstream".to_string()), ("host.b".to_string(), String::new())]
    );
}

#[test]
fn outcov_ac01a_gaps_txt_beside_the_fixtures_is_never_a_row() {
    let dir = std::env::temp_dir().join(format!("outcov-ac01a-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("GAPS.txt"), "host.x some reason\n").unwrap();
    std::fs::write(dir.join("host.y.jsonl"), "{\"ok\":true}\n\n{\"ok\":false}\n").unwrap();
    let committed = common::outsch_committed_rows(&dir);
    let keys: Vec<&String> = committed.keys().collect();
    assert_eq!(keys, ["host.y"], "only *.jsonl files are rows");
    assert_eq!(committed["host.y"].len(), 2);
    assert_eq!(common::outsch_gaps(&dir), vec![("host.x".to_string(), "some reason".to_string())]);
    let _ = std::fs::remove_dir_all(&dir);
}
