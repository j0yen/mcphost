//! PRD-mcphost-contract-output-schemas
//! AC1 (P0) -- Given the responses captured by the suite for every canonical
//! host tool, When each tool's rows are checked against its declared schema,
//! Then <= 10 % of tools need a `oneOf` of more than 3 branches; the test
//! prints the over-budget tools and fails the PRD's premise if the count
//! exceeds 18.

use crate::common;

use mcphost::output_schemas::{max_union_branches, output_schema_for, registered_tools};
use serde_json::{Value, json};

#[test]
fn at_most_ten_percent_of_tools_need_a_oneof_of_more_than_three_branches() {
    let tools = registered_tools();
    assert!(!tools.is_empty(), "no output schemas registered");
    let rows = common::outsch_rows();

    let mut over_budget = Vec::new();
    let mut failures = Vec::new();
    for tool in &tools {
        let schema = Value::Object((*output_schema_for(tool).expect("registered")).clone());
        let branches = max_union_branches(&schema);
        if branches > 3 {
            over_budget.push(format!("{tool} ({branches} branches)"));
        }
        let validator = jsonschema::validator_for(&schema).unwrap_or_else(|e| panic!("{tool}: schema invalid: {e}"));
        for row in rows.get(*tool).into_iter().flatten() {
            if let Err(e) = validator.validate(row) {
                failures.push(format!("{tool}: {e}"));
            }
        }
    }
    println!("tools needing a oneOf of more than 3 branches ({}): {over_budget:?}", over_budget.len());
    assert!(failures.is_empty(), "captured rows rejected by their schema:\n{}", failures.join("\n"));

    let allowed = tools.len() / 10;
    assert!(
        over_budget.len() <= 18,
        "premise falsified: {} tools need a oneOf of more than 3 branches (limit 18): {over_budget:?}",
        over_budget.len()
    );
    assert!(
        over_budget.len() <= allowed,
        "{} of {} tools exceed the 3-branch budget (10 % = {allowed}): {over_budget:?}",
        over_budget.len(),
        tools.len()
    );
}

// The coverage gate counts the COMMITTED corpus only. It used to count
// `outsch_rows()`, which adds `target/outsch/`, and the harness fills that
// directory only from suites that ran earlier in the same checkout. A CI core
// shard runs a weight-chosen subset of the suites, so the same head passed or
// failed depending on which suites shared its shard (PRD-mcphost-outsch-ac01-
// coverage-diagnosable, PR 575, names the defect; this keeps its rule: 80 %
// unchanged, committed rows only, the failure names the gap).
#[test]
fn the_corpus_covers_at_least_eighty_percent_of_registered_tools() {
    match common::outsch_coverage_report(&registered_tools(), &common::outsch_committed_rows()) {
        Ok(summary) => println!("{summary}"),
        Err(e) => panic!("{e}"),
    }
}

#[test]
fn the_coverage_verdict_ignores_what_target_outsch_holds() {
    let scratch = std::env::temp_dir().join(format!("outsch-ac01-capture-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let (empty, polluted) = (scratch.join("empty"), scratch.join("polluted"));
    std::fs::create_dir_all(&empty).unwrap();
    std::fs::create_dir_all(&polluted).unwrap();
    let tools = registered_tools();
    for tool in &tools {
        std::fs::write(polluted.join(format!("{tool}.jsonl")), "{}\n").unwrap();
    }
    let fixtures = common::outsch_fixture_dir();
    let committed = common::outsch_coverage_report(&tools, &common::outsch_committed_rows_in(&fixtures));
    // The union the gate used to count: a capture dir with a row for every
    // tool reads as full coverage, an empty one as the committed share.
    let union_polluted = common::outsch_coverage_report(&tools, &common::outsch_rows_in(&fixtures, &polluted));
    let union_empty = common::outsch_coverage_report(&tools, &common::outsch_rows_in(&fixtures, &empty));
    let _ = std::fs::remove_dir_all(&scratch);
    assert_eq!(union_empty, committed, "an empty capture dir adds nothing");
    let full = format!("{0}/{0} ", tools.len());
    assert!(union_polluted.as_ref().is_ok_and(|s| s.starts_with(&full)), "the old union reads a polluted dir as full coverage: {union_polluted:?}");
    assert_eq!(
        common::outsch_coverage_report(&tools, &common::outsch_committed_rows()),
        committed,
        "the gate reads the committed corpus, never the capture dir"
    );
}

#[test]
fn the_coverage_report_names_the_counts_and_every_uncovered_tool() {
    let tools = ["t.a", "t.b", "t.c", "t.d", "t.e", "t.f", "t.g", "t.h", "t.i", "t.j"];
    let rows = |n: usize| -> std::collections::BTreeMap<String, Vec<Value>> {
        tools[..n].iter().map(|t| (t.to_string(), vec![json!({})])).collect()
    };
    let err = common::outsch_coverage_report(&tools, &rows(7)).expect_err("7/10 is under 80 %");
    assert!(err.starts_with("7/10 "), "the counts lead the text: {err}");
    assert!(err.contains("need 8") && err.contains("under 80 %"), "{err}");
    assert!(err.contains(r#"["t.h", "t.i", "t.j"]"#), "every uncovered tool, sorted: {err}");
    assert!(!err.contains("\"t.a\""), "a covered tool is never listed: {err}");
    let ok = common::outsch_coverage_report(&tools, &rows(8)).expect("8/10 is 80 %");
    assert!(ok.starts_with("8/10 ") && ok.contains(r#"["t.i", "t.j"]"#), "{ok}");
    let mut empty_rows = rows(8);
    empty_rows.insert("t.h".to_string(), Vec::new());
    let err = common::outsch_coverage_report(&tools, &empty_rows).expect_err("a file with no rows covers nothing");
    assert!(err.starts_with("7/10 "), "{err}");
    let mut unregistered = rows(7);
    unregistered.insert("not.registered".to_string(), vec![json!({})]);
    let err = common::outsch_coverage_report(&tools, &unregistered).expect_err("a row for an unregistered tool covers nothing");
    assert!(err.starts_with("7/10 "), "{err}");
    let names: Vec<String> = (0..50).map(|i| format!("t.{i:02}")).collect();
    let many: Vec<&str> = names.iter().map(String::as_str).collect();
    let thirty: std::collections::BTreeMap<String, Vec<Value>> = many[..30].iter().map(|t| (t.to_string(), vec![json!({})])).collect();
    assert!(common::outsch_coverage_report(&many, &thirty).is_err_and(|e| e.starts_with("30/50 ")), "30/50 is 60 %");
}

#[test]
fn an_invalid_captured_row_still_fails_schema_validation() {
    // Rows from the capture dir are still VALIDATED (never counted): a row that
    // breaks its tool's schema is rejected, wherever it came from.
    let scratch = std::env::temp_dir().join(format!("outsch-ac01-invalid-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).unwrap();
    std::fs::write(scratch.join("host.whoami.jsonl"), format!("{}\n", json!({"not": "a whoami"}))).unwrap();
    let rows = common::outsch_rows_in(&common::outsch_fixture_dir(), &scratch);
    let _ = std::fs::remove_dir_all(&scratch);
    let schema = Value::Object((*output_schema_for("host.whoami").expect("registered")).clone());
    let validator = jsonschema::validator_for(&schema).expect("valid schema");
    let bad = rows["host.whoami"].iter().filter(|r| !validator.is_valid(r)).count();
    assert_eq!(bad, 1, "exactly the planted row is rejected");
}

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
