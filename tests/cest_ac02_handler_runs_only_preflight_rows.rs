//! PRD-mcphost-call-cost-estimate
//! AC2 -- Given `src/handler.rs`, When the drift test scans it, Then every
//! admission check reached by `call_published_tool` is a `PREFLIGHT_CHECKS`
//! row and no direct call to `check_calls_quota` or `resolve_and_validate`
//! exists outside `src/preflight.rs`.

use mcphost::preflight::{PREFLIGHT_CHECKS, Stage};

/// Called by name nowhere in `src/handler.rs`: the quota and budget checks
/// live in `src/preflight.rs` as `PREFLIGHT_CHECKS` rows.
const FILE_WIDE: &[&str] = &["check_calls_quota(", "resolve_and_validate("];

/// The admission primitives a `PREFLIGHT_CHECKS` row wraps. Reaching any of
/// them straight from the call path (`host_tool_call`, `call_published_tool`,
/// the dry run) would be a check the other path cannot see. Other handler
/// methods use some of these for unrelated reads (`host.tool_test` validates
/// args and deliberately skips the quota), so this list applies per function.
const ADMISSION_PRIMITIVES: &[&str] = &[
    "count_calls_since(",
    "get_caller_limit(",
    "count_caller_calls_since(",
    "validator_for(",
    "quota_caller(",
    "quota_exceeded(",
    "admission_capacity(",
];

/// Non-comment source lines, with their 1-based numbers.
fn code_lines(src: &str) -> Vec<(usize, &str)> {
    src.lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l))
        .filter(|(_, l)| !l.trim_start().starts_with("//"))
        .collect()
}

/// The text of `fn <name>(...) {...}` up to the next sibling method.
fn fn_body<'a>(src: &'a str, name: &str) -> &'a str {
    let start = src
        .find(&format!("async fn {name}("))
        .unwrap_or_else(|| panic!("handler.rs has no fn {name}"));
    let rest = &src[start..];
    let end = rest[1..]
        .find("\n    async fn ")
        .into_iter()
        .chain(rest[1..].find("\n    fn "))
        .min()
        .map_or(rest.len(), |e| e + 1);
    &rest[..end]
}

/// Every code line of `src` containing one of `needles`.
fn direct_admission_calls(src: &str, needles: &[&str]) -> Vec<String> {
    code_lines(src)
        .into_iter()
        .filter(|(_, l)| needles.iter().any(|p| l.contains(p)))
        .map(|(n, l)| format!("{n}: {}", l.trim()))
        .collect()
}

#[test]
fn handler_has_no_direct_admission_call_outside_the_preflight_list() {
    let handler = include_str!("../src/handler.rs");
    let found = direct_admission_calls(handler, FILE_WIDE);
    assert!(found.is_empty(), "src/handler.rs calls an admission check directly:\n{}", found.join("\n"));

    for name in ["host_tool_call", "call_published_tool", "dry_run_call"] {
        let found = direct_admission_calls(fn_body(handler, name), ADMISSION_PRIMITIVES);
        assert!(found.is_empty(), "{name} runs an admission check outside PREFLIGHT_CHECKS:\n{}", found.join("\n"));
    }
}

#[test]
fn the_scanner_flags_a_direct_call() {
    let offending = "fn x() {\n    // self.check_calls_quota(t) in a comment is fine\n    self.check_calls_quota(t).await?;\n    crate::budget::resolve_and_validate(s, p, b)?;\n}\n";
    let found = direct_admission_calls(offending, FILE_WIDE);
    assert_eq!(found.len(), 2, "{found:?}");
}

#[test]
fn call_published_tool_reaches_admission_only_through_the_list() {
    let handler = include_str!("../src/handler.rs");
    let body = fn_body(handler, "call_published_tool");
    assert!(body.contains("preflight::resolve("), "call_published_tool must resolve through preflight");
    assert!(
        body.contains("preflight::run_stage(") && body.contains("Stage::Resolved"),
        "call_published_tool must run the Resolved rows of PREFLIGHT_CHECKS"
    );
    let host_call = fn_body(handler, "host_tool_call");
    assert!(
        host_call.contains("preflight::run_stage(") && host_call.contains("Stage::PreResolve"),
        "host_tool_call must run the PreResolve rows of PREFLIGHT_CHECKS"
    );
}

#[test]
fn the_list_is_the_five_checks_in_call_order() {
    let ids: Vec<&str> = PREFLIGHT_CHECKS.iter().map(|c| c.id).collect();
    assert_eq!(ids, ["budget_ceiling", "args_schema", "calls_per_day", "caller_limit", "capacity"]);
    // Every row runs in exactly one of the two stages the real path runs.
    assert!(PREFLIGHT_CHECKS.iter().all(|c| matches!(c.stage, Stage::PreResolve | Stage::Resolved)));
    // The dry run (host_tool_call's `dry_run_call`) walks the same two stages.
    let handler = include_str!("../src/handler.rs");
    let dry = fn_body(handler, "dry_run_call");
    assert!(dry.contains("Stage::PreResolve") && dry.contains("Stage::Resolved"));
}
