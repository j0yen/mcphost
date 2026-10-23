//! contract: reads registry handler::host_tool_descriptors
//! contract: reads contracts/host-output-observational-edits.v1.json
//! PRD-mcphost-success-result-matches-output-schema
//! AC8 (P0, after AC 4) -- Given the R4 edits applied to the composed
//! registry, When the `osmatch_ac08_sample_rows` test validates its sample
//! rows, Then a host.docs.get row without `text`, a pending-ask row from each
//! of the three ask verbs, a fitting host.estimate row and a host.call_trace
//! row with a `null` side each validate against the advertised `outputSchema`.

use mcphost::handler::host_tool_descriptors;
use mcphost::kinds::KindRegistry;
use serde_json::{Value, json};

/// First captured row of `tool` (the committed corpus), the base a sample edits.
fn base(tool: &str) -> Value {
    let path = format!("{}/tests/fixtures/outsch/{tool}.jsonl", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(text.lines().next().expect("a captured row")).expect("row is JSON")
}

fn pending(mut ask: Value) -> Value {
    ask["status"] = json!("pending");
    ask["answered_unix"] = Value::Null;
    ask["choice"] = Value::Null;
    ask
}

/// `(tool, sample row)`: the shapes the host emits that the corpus never held.
fn samples() -> Vec<(&'static str, Value)> {
    let mut docs = base("host.docs.get");
    docs.as_object_mut().expect("object").remove("text");

    let mut ask_get = pending(base("host.ask.get"));
    ask_get["events"] = json!([{"at_unix": 1791809343, "kind": "created"}]);
    let ask_list_row = pending(base("host.ask.list")["asks"][0].clone());

    let mut estimate = base("host.estimate");
    estimate["fits"] = json!(true);
    estimate["refusal"] = Value::Null;

    let mut trace = base("host.call_trace");
    trace["replay"] = json!({
        "call_id": "9",
        "result": {"call_id": "9", "error_code": "tool_exception", "message": "boom", "tail": ""},
        "verdict": "will_fail",
        "diff": {"ok": [true, false], "error_code": [null, "tool_exception"]},
    });
    let mut trace_both_ok = trace.clone();
    trace_both_ok["replay"]["diff"] = json!({"ok": [true, true], "error_code": [null, null]});

    vec![
        ("host.docs.get", docs),
        (
            "host.ask.wait",
            json!({"ask_id": "a1", "status": "pending", "waited_ms": 30000, "next_call": "host.ask.wait", "next_args": {"ask_id": "a1"}}),
        ),
        ("host.ask.get", ask_get),
        ("host.ask.list", json!({"asks": [ask_list_row]})),
        ("host.estimate", estimate),
        ("host.call_trace", trace),
        ("host.call_trace", trace_both_ok),
    ]
}

fn validator(schema: &serde_json::Map<String, Value>) -> jsonschema::Validator {
    jsonschema::validator_for(&Value::Object(schema.clone())).expect("schema compiles")
}

#[test]
fn every_sample_row_validates_against_the_advertised_output_schema() {
    let tools = host_tool_descriptors(&KindRegistry::with_builtin());
    let mut problems = Vec::new();
    for (tool, row) in samples() {
        let advertised = tools.iter().find(|t| t.name.as_ref() == tool).expect("served");
        let schema = advertised.output_schema.as_ref().expect("outputSchema");
        if let Some(e) = validator(schema).iter_errors(&row).next() {
            problems.push(format!("{tool}: {}: {e}\n  row: {row}", e.instance_path));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn each_sample_row_was_rejected_by_the_schema_at_the_cited_tree() {
    let cited: Value = serde_json::from_str(include_str!("fixtures/osmatch/cited-output-schemas.json")).expect("fixture");
    for (tool, row) in samples() {
        let schema = cited["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .find(|t| t["name"] == tool)
            .map(|t| t["output_schema"].as_object().expect("schema").clone())
            .expect("cited schema");
        assert!(!validator(&schema).is_valid(&row), "{tool}: the sample was valid before this PRD, so it proves nothing: {row}");
    }
}
