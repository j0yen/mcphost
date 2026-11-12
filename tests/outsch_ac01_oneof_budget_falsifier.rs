//! PRD-mcphost-contract-output-schemas
//! AC1 (P0) -- Given the responses captured by the suite for every canonical
//! host tool, When each tool's rows are checked against its declared schema,
//! Then <= 10 % of tools need a `oneOf` of more than 3 branches; the test
//! prints the over-budget tools and fails the PRD's premise if the count
//! exceeds 18.

use crate::common;

use mcphost::output_schemas::{output_schema_for, registered_tools};
use serde_json::{Value, json};

#[test]
fn at_most_ten_percent_of_tools_need_a_oneof_of_more_than_three_branches() {
    let rows = common::outsch_rows(&common::outsch_fixture_dir(), &common::outsch_capture_dir());
    if let Err(e) = common::branch_budget_verdict(&rows) {
        panic!("{e}");
    }
}

// The coverage gate (the corpus covers at least 80 % of registered tools)
// moved to `outcov_ac02_*` / `outcov_ac01a_coverage_report.rs`
// (PRD-mcphost-outsch-ac01-coverage-diagnosable): it reads the committed
// fixtures alone, so its verdict is the same in every shard. This file's
// branch-budget test validates the union of the committed and captured rows
// (rows are checked here, never counted).

// ---- hotfix output-schema-nullable-and-optional-cursor ----
// Hotfix: prod mcphost rejected its own `structuredContent` ("Structured
// content does not match the tool's output schema", 538 explore-leg rows).
// The schemas in `contracts/host-output-schemas.v1.json` are inferred from
// the response corpus, which cannot tell an `Option<T>` (a `json!` null) from
// a `T`, and cannot see a cursor the server omits on the last page. So the
// contract is DERIVED from the corpus by rules in `output_schemas`, and this
// test proves the rules hold for EVERY registered tool:
//
// 1. a property is never declared null-only (a null-only observation carries
//    no type information; the next real value would be rejected);
// 2. every non-enum property that is not `required` accepts `null` (the server emits
//    null for an absent Option) -- walked recursively through `properties`
//    and `items`, so the failure names the tool and the JSON pointer;
// 3. a `*cursor` key is never `required` (the server omits it with no next
//    page);
// 4. verbatim prod `structuredContent` captures of the failing tools
//    validate against their own schema.

fn walk(tool: &str, schema: &Value, pointer: &str, errors: &mut Vec<String>) {
    let Some(obj) = schema.as_object() else { return };
    let required: Vec<&str> = mcphost::output_schemas::live_required(obj);
    for key in required.iter().filter(|k| k.ends_with("cursor")) {
        errors.push(format!("{tool} {pointer}/{key}: a cursor is omitted on the last page and must not be required"));
    }
    if let Some(props) = obj.get("properties").and_then(Value::as_object) {
        for (name, sub) in props {
            let at = format!("{pointer}/{name}");
            if sub.get("type").and_then(Value::as_str) == Some("null") {
                errors.push(format!("{tool} {at}: declared null-only, any real value is rejected"));
            } else if !required.contains(&name.as_str()) && sub.get("enum").is_none() {
                let accepts_null = jsonschema::validator_for(sub).is_ok_and(|v| v.is_valid(&Value::Null));
                if !accepts_null {
                    errors.push(format!("{tool} {at}: optional property rejects null ({})", sub.get("type").unwrap_or(&Value::Null)));
                }
            }
            walk(tool, sub, &at, errors);
        }
    }
    if let Some(items) = obj.get("items") {
        walk(tool, items, &format!("{pointer}/*"), errors);
    }
}

#[test]
fn every_output_schema_tolerates_null_options_and_omitted_cursors() {
    let tools = registered_tools();
    assert!(!tools.is_empty(), "no output schemas registered");
    let mut errors = Vec::new();
    for tool in &tools {
        let schema = Value::Object((*output_schema_for(tool).expect("registered")).clone());
        walk(tool, &schema, "", &mut errors);
    }
    assert!(errors.is_empty(), "{} output-schema defects:\n{}", errors.len(), errors.join("\n"));
}

#[test]
fn verbatim_prod_responses_validate_against_their_own_schema() {
    // structuredContent captured from prod 0.108.27 (joe-test tenant, tools/call)
    // under tests/fixtures/outsch_prod/. `set` replaces a null the server emits
    // today with the value it carries when the optional is present (e.g. an
    // OAuth end user on a run): the case the schema inferred from null-only rows
    // rejected as "must be null".
    type Case<'a> = (&'a str, Vec<(&'a str, Value)>);
    let cases: Vec<Case> = vec![
        ("host.catalog.get", vec![]),
        ("host.catalog.search", vec![("/tools/0/group", json!("team"))]),
        ("host.msg.wait", vec![]),
        ("host.runs.list", vec![]),
        ("host.runs.get", vec![("/end_user", json!({"issuer": "https://issuer.example", "method": "oauth", "subject": "u"}))]),
    ];
    let mut errors = Vec::new();
    for (tool, set) in &cases {
        let path = format!("{}/tests/fixtures/outsch_prod/{tool}.json", env!("CARGO_MANIFEST_DIR"));
        let mut row: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}")))
            .unwrap_or_else(|e| panic!("{path}: {e}"));
        for (pointer, value) in set {
            *row.pointer_mut(pointer).unwrap_or_else(|| panic!("{tool}: no {pointer} in fixture")) = value.clone();
        }
        let schema = Value::Object((*output_schema_for(tool).unwrap_or_else(|| panic!("{tool}: no schema"))).clone());
        let validator = jsonschema::validator_for(&schema).unwrap_or_else(|e| panic!("{tool}: schema invalid: {e}"));
        for e in validator.iter_errors(&row) {
            errors.push(format!("{tool} {}: {e}", e.instance_path));
        }
    }
    assert!(errors.is_empty(), "prod responses rejected by their own schema:\n{}", errors.join("\n"));
}
