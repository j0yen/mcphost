//! PRD-mcphost-onboarding-dead-ends-zero
//! AC1 — Given every `AppError` variant and the five kind-level codes, When
//! `next_call` is evaluated with an empty `ErrorCtx`, Then every one returns a
//! `NextCall` whose `tool` is in `tools/list` and the four 10-08 families
//! (`args_invalid`, `tool_exception`, plan-ceiling, `upstream_status`) each
//! name a tool other than `host.quickstart` (the falsifier as a test).

use mcphost::errors::{AppError, ErrorCtx};
use mcphost::handler::host_tool_descriptors;
use mcphost::kinds::KindRegistry;
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn structured(code: &'static str) -> AppError {
    AppError::Structured { code, message: "x".into(), data: Value::Null }
}

#[test]
fn ndead_ac01() {
    let kinds = KindRegistry::new();
    let descriptors = host_tool_descriptors(&kinds);
    let mut listed: BTreeSet<String> = descriptors.iter().map(|t| t.name.to_string()).collect();
    listed.extend(mcphost::tool_aliases::TOOL_ALIASES.iter().map(|a| a.alias.to_string()));

    let five = ["args_invalid", "tool_exception", "upstream_status", "host_not_allowed", "plan_limit"];
    let mut errors = mcphost::errors::all_variants_for_test();
    errors.extend(five.iter().map(|c| structured(c)));
    assert!(errors.len() >= 39 + 5, "variant list shrank: {}", errors.len());

    let ctx = ErrorCtx::default();
    for err in &errors {
        let next = err.next_call(&ctx);
        assert!(
            listed.contains(next.tool),
            "{} -> {} is not in tools/list",
            err.code(),
            next.tool
        );
        let wire = next.to_json();
        assert_eq!(wire["tool"], next.tool);
        assert!(wire["args"].is_object(), "{}: args must be an object: {wire}", err.code());
    }

    // The falsifier: each 10-08 family names something other than the generic
    // quickstart default.
    for code in ["args_invalid", "tool_exception", "plan_limit", "upstream_status"] {
        let next = structured(code).next_call(&ctx);
        assert_ne!(next.tool, "host.quickstart", "{code} still dead-ends at the quickstart");
    }
    // And through the envelope: the field is on the wire.
    let data = structured("plan_limit").into_error_data().data.expect("data");
    assert_eq!(data["next_call"]["tool"], json!("billing.plans"), "{data}");
}
