//! PRD-mcphost-generated-dispatch-table-stack-budget
//! AC5 — Given the guard `verbfwd_ac08_forward_stack_budget` with its registry
//! walk and `SHALLOW` list unchanged, When `scripts/fwd-stack-measure.sh all`
//! binary-searches all 230 ids on the debug build, Then the maximum is
//! <= 1,048,576 B, `FORWARD_STACK_BUDGET` equals that maximum + 25 % and is
//! <= 1,310,720 B, its doc comment names the maximum, its ids and the date, and
//! the guard passes at that budget (`verbfwd_ac08` itself runs it).
//! contract: reads registry handler::host_tool_descriptors

use mcphost::handler::FORWARD_STACK_BUDGET;
use std::process::Command;

const MAX_CEILING: usize = 1_048_576;
const BUDGET_CEILING: usize = 1_310_720;

/// The doc comment above `pub const FORWARD_STACK_BUDGET`.
fn budget_doc() -> String {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/handler.rs")).expect("handler.rs");
    let end = src.find("pub const FORWARD_STACK_BUDGET").expect("the constant");
    let docs: Vec<&str> = src[..end]
        .lines()
        .rev()
        .take_while(|l| l.starts_with("///"))
        .collect();
    docs.into_iter().rev().collect::<Vec<_>>().join("\n")
}

/// `1,029,120` -> 1029120.
fn parse_bytes(s: &str) -> usize {
    s.chars().filter(char::is_ascii_digit).collect::<String>().parse().expect("a byte count")
}

/// The measured maximum the doc comment records: "maximum over all 230 forwardable ids is N B".
fn documented_max(doc: &str) -> usize {
    let after = doc.split("maximum over all 230 forwardable ids is ").nth(1).expect("doc names the measured maximum");
    parse_bytes(after.split(" B").next().expect("bytes"))
}

#[test]
fn fwdtab_ac05_budget_is_the_documented_maximum_plus_25_percent() {
    let doc = budget_doc();
    let max = documented_max(&doc);
    assert!(max <= MAX_CEILING, "documented maximum {max} B > {MAX_CEILING} B");
    assert_eq!(FORWARD_STACK_BUDGET, max + max / 4, "FORWARD_STACK_BUDGET is the maximum + 25 %");
    assert!(FORWARD_STACK_BUDGET <= BUDGET_CEILING, "{FORWARD_STACK_BUDGET} B > {BUDGET_CEILING} B");
    // The comment names the maximum, its ids and the date.
    assert!(doc.contains("`test:host.trigger.set`") && doc.contains("`call:host.whoami`"), "doc names the ids: {doc}");
    assert!(doc.contains("2026-10-18"), "doc names the date: {doc}");
}

#[test]
fn fwdtab_ac05_guard_keeps_its_registry_walk_and_shallow_list() {
    // The guard still derives its ids from the registry and keeps the SHALLOW skip
    // list; nothing in it is hand-listed per verb, and it reads the budget from
    // handler.rs.
    let guard = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/verbfwd_ac08_forward_stack_budget.rs"))
        .expect("the guard");
    assert!(guard.contains("mcphost::handler::host_tool_descriptors(&kinds)"), "registry walk");
    assert!(guard.contains("const SHALLOW: &[&str]"), "SHALLOW list");
    assert!(guard.contains("use mcphost::handler::FORWARD_STACK_BUDGET;"), "budget read from handler.rs");
}

#[test]
fn fwdtab_ac05_a_measured_id_fits_within_the_documented_maximum() {
    // Debug-profile claim; the script's `--release` mode is AC7's.
    if !cfg!(debug_assertions) {
        return;
    }
    let max = documented_max(&budget_doc());
    let out = Command::new("bash")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["scripts/fwd-stack-measure.sh", "call:host.whoami"])
        .output()
        .expect("run fwd-stack-measure.sh");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "fwd-stack-measure.sh failed: {stdout} {}", String::from_utf8_lossy(&out.stderr));
    let measured: usize = stdout
        .trim()
        .strip_prefix("min call:host.whoami ")
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("unexpected measure output: {stdout}"));
    println!("call:host.whoami minimum stack: {measured} B (documented maximum {max} B)");
    assert!(measured <= max, "call:host.whoami needs {measured} B > the documented maximum {max} B");
}
