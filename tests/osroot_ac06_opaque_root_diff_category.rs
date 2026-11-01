//! contract: reads registry handler::host_tool_descriptors
//! PRD-mcphost-tool-call-output-root-opaque
//! AC6 (P1) -- Given a before/after fixture with a removed tool root, When
//! `api_contract::diff` runs, Then it reports the removal under a new
//! `opaque_root` category; against the cited tree it reports zero narrowed
//! types and lists the removed host.tool.call and host.tool.test roots under
//! opaque_root.

use mcphost::api_contract::{Violation, diff, dump_contract};
use mcphost::kinds::KindRegistry;
use mcphost::output_schemas::FORWARDING_ROOTS;
use serde_json::{Value, json};

fn contract(tool: &str, properties: Value) -> Value {
    json!({"tools": [{"name": tool, "input_schema": {}, "output_schema": {"type": "object", "properties": properties}}]})
}

#[test]
fn osroot_ac06_a_removed_root_property_is_reported_under_opaque_root_not_removed() {
    let before = contract("host.tool.call", json!({"checked_at": {"type": "integer"}, "forwarded_to": {"type": "string"}}));
    let after = contract("host.tool.call", json!({"forwarded_to": {"type": "string"}}));
    let found = diff(&before, &after, &[], mcphost::state::now_unix());
    assert_eq!(
        found,
        vec![Violation::OpaqueRoot { path: "host.tool.call.output".to_string(), removed: vec!["checked_at".to_string()] }]
    );

    // A tool that is not an opaque root still reports a plain removal.
    let before = contract("host.runs.get", json!({"checked_at": {"type": "integer"}}));
    let after = contract("host.runs.get", json!({}));
    let found = diff(&before, &after, &[], mcphost::state::now_unix());
    assert!(matches!(&found[..], [Violation::Removed { path }] if path == "host.runs.get.output.checked_at"), "{found:?}");
}

#[test]
fn osroot_ac06_against_the_cited_tree_nothing_narrows_and_both_roots_are_listed() {
    let cited: Value = serde_json::from_str(include_str!("fixtures/osmatch/cited-output-schemas.json")).expect("fixture");
    let live = dump_contract(&KindRegistry::with_builtin());
    let findings = diff(&cited, &live, &[], mcphost::state::now_unix());

    // The only narrowings left are the `required` keys PRD
    // success-result-matches-output-schema drops by named edit; no type narrowed.
    let narrowed_types: Vec<_> = findings
        .iter()
        .filter(|f| matches!(f, Violation::Narrowed { detail, .. } if detail != "no longer always present"))
        .collect();
    assert!(narrowed_types.is_empty(), "{narrowed_types:?}");

    for tool in FORWARDING_ROOTS {
        let root = findings
            .iter()
            .find_map(|f| match f {
                Violation::OpaqueRoot { path, removed } if path == &format!("{tool}.output") => Some(removed),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{tool} root not listed under opaque_root: {findings:?}"));
        assert!(root.iter().any(|k| k == "msg") && root.iter().any(|k| k == "payload"), "{tool}: {root:?}");
    }
    assert!(
        !findings.iter().any(|f| matches!(f, Violation::Removed { path } if FORWARDING_ROOTS.iter().any(|t| path.starts_with(&format!("{t}.output."))))),
        "a root property removal must be reported as opaque_root, not removed: {findings:?}"
    );
}
