//! PRD-mcphost-onboarding-dead-ends-zero AC1 (P0) -- Given every `AppError`
//! variant and the five kind-level codes, When `next_call` is evaluated with
//! an empty `ErrorCtx`, Then every one returns a `NextCall` whose `tool` is in
//! `tools/list`, and the four 10-08 families (`args_invalid`, `tool_exception`,
//! plan-ceiling, `upstream_status`) each name a tool other than
//! `host.quickstart` (the falsifier as a test).

//! contract: reads registry handler::host_tool_descriptors

use mcphost::errors::{AppError, all_variants_for_test};
use mcphost::handler::{host_tool_descriptors, llms_txt_tool_names};
use mcphost::kinds::production_registry;
use mcphost::next_call::ErrorCtx;
use serde_json::Value;
use std::collections::BTreeSet;

const KIND_LEVEL_FIVE: [&str; 5] =
    ["args_invalid", "tool_exception", "upstream_status", "host_not_allowed", "plan_limit"];

fn structured(code: &'static str) -> AppError {
    AppError::Structured { code, message: String::new(), data: Value::Null }
}

#[test]
fn ndead_ac01_every_variant_and_kind_code_names_a_listed_tool() {
    let registry = production_registry();
    let mut listed: BTreeSet<String> = llms_txt_tool_names(&registry).into_iter().collect();
    listed.insert("signup".to_string());
    let descriptors = host_tool_descriptors(&registry);

    let mut errors = all_variants_for_test();
    errors.extend(KIND_LEVEL_FIVE.iter().map(|c| structured(c)));
    assert!(errors.len() >= 39 + 5, "expected every variant plus the kind codes, got {}", errors.len());

    let ctx = ErrorCtx::default();
    for err in &errors {
        let next = err.next_call(&ctx);
        // A deprecated alias is a valid name (PRD-b R4: host_tools ∪ aliases).
        let canonical = mcphost::tool_aliases::resolve(next.tool).unwrap_or(next.tool);
        assert!(listed.contains(canonical), "{} -> {} is not in tools/list", err.code(), next.tool);
        // args keys must be properties of the named tool's input schema.
        let tool = descriptors.iter().find(|t| t.name.as_ref() == canonical);
        if let (Some(tool), Some(args)) = (tool, next.args.as_object()) {
            let props = tool.input_schema.get("properties").and_then(Value::as_object);
            for key in args.keys() {
                assert!(
                    props.is_some_and(|p| p.contains_key(key)),
                    "{} -> {} arg '{key}' is not in its input schema",
                    err.code(),
                    next.tool
                );
            }
        }
    }
}

#[test]
fn ndead_ac01_four_families_name_a_tool_other_than_quickstart() {
    let ctx = ErrorCtx::default();
    let families = [
        ("args_invalid", structured("args_invalid")),
        ("tool_exception", structured("tool_exception")),
        ("plan-ceiling", structured("plan_limit")),
        ("plan-ceiling (variant)", AppError::ShareQuotaExceeded { limit: 1, used: 1 }),
        ("upstream_status", structured("upstream_status")),
    ];
    for (family, err) in families {
        let next = err.next_call(&ctx);
        assert_ne!(next.tool, "host.quickstart", "family {family} still points at host.quickstart");
    }
}
