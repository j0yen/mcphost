//! PRD-mcphost-contract-output-schemas
//! AC6 (P0) -- Given a committed contract and a live dump where
//! `host.runs.get.output_schema` lost `started_at` with no
//! `deprecations.json` entry, When `diff` runs, Then it returns one
//! `Violation` whose path is `host.runs.get.output.started_at`.

use mcphost::api_contract::{Deprecation, Violation, diff, dump_contract};
use mcphost::kinds::KindRegistry;
use serde_json::{Value, json};

fn tool_mut<'a>(contract: &'a mut Value, name: &str) -> &'a mut Value {
    contract["tools"]
        .as_array_mut()
        .expect("tools")
        .iter_mut()
        .find(|t| t["name"] == name)
        .unwrap_or_else(|| panic!("{name} in contract"))
}

fn now() -> i64 {
    mcphost::state::now_unix()
}

#[test]
fn a_removed_output_property_is_one_violation_naming_its_path() {
    let live = dump_contract(&KindRegistry::with_builtin());
    // The committed contract still described `started_at`; the live schema
    // (this tree's `host.runs.get` has `started_unix`) no longer does.
    let mut committed = live.clone();
    tool_mut(&mut committed, "host.runs.get")["output_schema"]["properties"]["started_at"] =
        json!({"type": "integer"});

    let violations = diff(&committed, &live, &[], now());
    assert_eq!(
        violations,
        vec![Violation::Removed { path: "host.runs.get.output.started_at".to_string() }],
        "{violations:?}"
    );
}

#[test]
fn removing_a_real_output_field_is_flagged_and_a_valid_deprecation_excuses_it_after_sunset() {
    let committed = dump_contract(&KindRegistry::with_builtin());
    let mut live = committed.clone();
    tool_mut(&mut live, "host.runs.get")["output_schema"]["properties"]
        .as_object_mut()
        .expect("properties")
        .remove("started_unix");
    let flagged = diff(&committed, &live, &[], now());
    assert_eq!(flagged.len(), 1, "{flagged:?}");
    assert_eq!(flagged[0].path(), "host.runs.get.output.started_unix");

    let entry = Deprecation {
        path: "host.runs.get.output.started_unix".into(),
        since: "2026-01-01".into(),
        sunset: "2026-03-02".into(),
        replacement: "finished_unix".into(),
        narrowing_only: false,
    };
    assert!(diff(&committed, &live, &[entry], now()).is_empty());
}

#[test]
fn a_changed_output_type_is_flagged_and_an_added_field_or_missing_old_schema_is_not() {
    let committed = dump_contract(&KindRegistry::with_builtin());

    let mut retyped = committed.clone();
    tool_mut(&mut retyped, "host.runs.get")["output_schema"]["properties"]["run_id"]["type"] = json!("boolean");
    let v = diff(&committed, &retyped, &[], now());
    assert!(matches!(&v[..], [Violation::Narrowed { path, .. }] if path == "host.runs.get.output.run_id"), "{v:?}");

    let mut grown = committed.clone();
    tool_mut(&mut grown, "host.runs.get")["output_schema"]["properties"]["brand_new"] = json!({"type": "string"});
    assert!(diff(&committed, &grown, &[], now()).is_empty());

    // A contract written before this PRD has no output_schema anywhere.
    let mut old = committed.clone();
    for t in old["tools"].as_array_mut().expect("tools") {
        t.as_object_mut().expect("entry").remove("output_schema");
    }
    assert!(diff(&old, &committed, &[], now()).is_empty());
}
