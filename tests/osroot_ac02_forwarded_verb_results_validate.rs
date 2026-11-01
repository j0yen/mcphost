//! contract: reads registry handler::host_tool_descriptors
//! PRD-mcphost-tool-call-output-root-opaque
//! AC2 (P0) -- Given every verb in the `verbforward` allowlist, When the osroot
//! forwarding test validates a schema-valid sample of that verb's result plus
//! `forwarded_to` and `client_tool` against `host.tool.call`'s advertised
//! `outputSchema`, Then every sample validates, and adding a verb to the
//! allowlist adds a case without editing the test.

use crate::common;
use mcphost::handler::host_tool_descriptors;
use mcphost::kinds::KindRegistry;
use mcphost::kinds::example::example_from_schema;
use mcphost::verbforward;
use serde_json::{Map, Value};

fn validator(schema: &Map<String, Value>) -> jsonschema::Validator {
    jsonschema::validator_for(&Value::Object(schema.clone())).expect("schema compiles")
}

/// Every canonical verb `host.tool.call` forwards: the registry's `host.*`
/// names (aliases folded away) minus `verbforward::FORWARD_DENYLIST`.
pub fn forwardable_verbs(kinds: &KindRegistry) -> Vec<String> {
    let mut verbs: Vec<String> = host_tool_descriptors(kinds)
        .into_iter()
        .map(|t| t.name.to_string())
        .filter(|n| n.starts_with("host.") && mcphost::tool_aliases::resolve(n).is_none())
        .filter(|n| !verbforward::is_denylisted(n))
        .collect();
    verbs.sort();
    verbs.dedup();
    verbs
}

/// The smallest instance of `schema`: every required property, by type.
fn minimal(schema: &Value) -> Value {
    if let Some(first) = schema.get("anyOf").and_then(Value::as_array).and_then(|b| b.first()) {
        return minimal(first);
    }
    if let Some(choice) = schema.get("enum").and_then(Value::as_array).and_then(|e| e.first()) {
        return choice.clone();
    }
    let kind = match schema.get("type") {
        Some(Value::String(t)) => t.as_str(),
        Some(Value::Array(ts)) => ts.iter().filter_map(Value::as_str).find(|t| *t != "null").unwrap_or("null"),
        _ => "string",
    };
    match kind {
        "object" => {
            let props = schema.get("properties").and_then(Value::as_object);
            let required = schema.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str);
            Value::Object(
                required
                    .map(|k| (k.to_string(), props.and_then(|p| p.get(k)).map_or(Value::Null, minimal)))
                    .collect(),
            )
        }
        "array" => Value::Array(Vec::new()),
        "integer" | "number" => Value::from(0),
        "boolean" => Value::Bool(false),
        "null" => Value::Null,
        _ => Value::from("x"),
    }
}

/// A result of `verb` its own advertised schema accepts: a captured row when
/// the corpus holds one, otherwise one generated from the schema.
pub fn sample_result(verb: &str, own: &Map<String, Value>, rows: &std::collections::BTreeMap<String, Vec<Value>>) -> Option<Value> {
    let own_validator = validator(own);
    // The live branch: variants (dry-run previews) are not a verb's ordinary result.
    let live = mcphost::output_schemas::live_view(&Value::Object(own.clone()));
    rows.get(verb)
        .into_iter()
        .flatten()
        .cloned()
        .chain([example_from_schema(&live).args, minimal(&live)])
        .find(|candidate| candidate.is_object() && own_validator.is_valid(candidate))
}

#[test]
fn osroot_ac02_every_forwardable_verb_result_validates_at_the_call_root() {
    let kinds = KindRegistry::with_builtin();
    let tools = host_tool_descriptors(&kinds);
    let advertised = |name: &str| {
        tools
            .iter()
            .find(|t| t.name.as_ref() == name)
            .and_then(|t| t.output_schema.as_ref())
            .unwrap_or_else(|| panic!("{name} advertises an outputSchema"))
            .clone()
    };
    let call = validator(&advertised("host.tool.call"));
    let rows = common::outsch_rows();

    let verbs = forwardable_verbs(&kinds);
    assert!(verbs.len() > 150, "only {} forwardable verbs", verbs.len());
    assert!(!verbs.iter().any(|v| v == "host.tool.call" || v == "host.tool.test"), "denylist applied");

    let mut problems = Vec::new();
    for verb in &verbs {
        let own = advertised(verb);
        let Some(mut sample) = sample_result(verb, &own, &rows) else {
            problems.push(format!("{verb}: no schema-valid sample result (captured or generated)"));
            continue;
        };
        let object = sample.as_object_mut().expect("object");
        object.insert("forwarded_to".to_string(), Value::from(verb.as_str()));
        object.insert("client_tool".to_string(), Value::from(verbforward::client_tool_name(verb)));
        if let Some(e) = call.iter_errors(&sample).next() {
            problems.push(format!("{verb}: {}: {e}", e.instance_path));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
