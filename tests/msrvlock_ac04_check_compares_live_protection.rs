//! PRD-mcphost-msrv-locked-required-check AC4 -- `--check` against a saved
//! protection JSON (`REQUIRED_CHECKS_JSON`): missing contexts and stale
//! extras exit 1 naming them; the exact 9 exit 0.

use std::path::Path;

use crate::msrvlock;
use msrvlock as support;

fn check(tag: &str, contexts: &[&str]) -> (i32, String) {
    let dir = support::scratch(tag);
    let json = dir.join("protection.json");
    let list: Vec<String> = contexts.iter().map(|c| format!("{c:?}")).collect();
    std::fs::write(
        &json,
        format!("{{\"strict\": false, \"contexts\": [{}]}}", list.join(", ")),
    )
    .unwrap();
    let o = support::run_script(
        &support::repo_root(),
        &["--check"],
        &[("REQUIRED_CHECKS_JSON", Path::new(&json))],
    );
    let _ = std::fs::remove_dir_all(&dir);
    (o.status.code().unwrap_or(-1), support::text(&o))
}

#[test]
fn msrvlock_ac04_two_current_entries_name_the_seven_missing() {
    let (code, out) = check(
        "ac04a",
        &["static analysis + core suites", "sandbox suites (all shards)"],
    );
    assert_eq!(code, 1, "{out}");
    let all = support::contexts();
    let missing: Vec<&str> = all
        .iter()
        .map(String::as_str)
        .filter(|c| *c != "static analysis + core suites" && *c != "sandbox suites (all shards)")
        .collect();
    assert_eq!(missing.len(), all.len() - 2);
    for c in missing {
        assert!(out.contains(c), "missing context not named: {c}\n{out}");
    }
    assert!(out.contains("msrv check"), "{out}");
}

#[test]
fn msrvlock_ac04_all_nine_exit_zero() {
    let all = support::contexts();
    let all: Vec<&str> = all.iter().map(String::as_str).collect();
    let (code, out) = check("ac04b", &all);
    assert_eq!(code, 0, "{out}");
}

#[test]
fn msrvlock_ac04_stale_extra_is_named() {
    let owned = support::contexts();
    let mut all: Vec<&str> = owned.iter().map(String::as_str).collect();
    all.push("ancient job that no longer exists");
    let (code, out) = check("ac04c", &all);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("ancient job that no longer exists"), "{out}");
}
