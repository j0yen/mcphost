//! PRD-mcphost-client-profile-registry AC1 — Given the committed
//! `CLIENT_PROFILES`, When the falsifier test runs, Then at least two
//! constraint columns hold >= 3 distinct values and every non-unknown row's
//! `doc_url` is listed for `scripts/www-check.sh --client-docs` (which asserts
//! HTTP 200 in CI); otherwise the PRD stops.

use std::collections::BTreeSet;
use std::process::Command;

use mcphost::client_profiles::{CLIENT_PROFILES, doc_urls};

#[test]
fn at_least_two_constraint_columns_hold_three_distinct_values() {
    let distinct = |f: &dyn Fn(&mcphost::client_profiles::ClientProfile) -> String| {
        CLIENT_PROFILES.iter().map(f).collect::<BTreeSet<_>>().len()
    };
    let columns = [
        ("tool_name_max", distinct(&|p| p.tool_name_max.to_string())),
        ("nested_depth_max", distinct(&|p| p.nested_depth_max.to_string())),
        ("structured_content", distinct(&|p| p.structured_content.to_string())),
        ("oauth", distinct(&|p| format!("{:?}", p.oauth))),
        ("result_bytes_max", distinct(&|p| p.result_bytes_max.to_string())),
    ];
    let rich: Vec<_> = columns.iter().filter(|(_, n)| *n >= 3).collect();
    assert!(
        rich.len() >= 2,
        "FALSIFIED: fewer than two constraint columns hold >= 3 distinct values: {columns:?} -- stop the PRD"
    );
}

#[test]
fn every_row_has_a_doc_url_and_the_check_script_lists_exactly_those_urls() {
    for p in &CLIENT_PROFILES {
        assert!(p.doc_url.starts_with("https://"), "{}: doc_url {:?}", p.id, p.doc_url);
        assert!(!p.checked.is_empty(), "{}: checked date missing", p.id);
    }
    let out = Command::new("bash")
        .arg("scripts/www-check.sh")
        .args(["--client-docs", "--list"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run www-check.sh");
    assert!(out.status.success());
    let listed: Vec<String> = String::from_utf8(out.stdout).unwrap().lines().map(str::to_string).collect();
    let table: Vec<String> = doc_urls().iter().map(|u| u.to_string()).collect();
    assert_eq!(listed, table, "www-check.sh --client-docs must check exactly the table's doc_urls");
}
