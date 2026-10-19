//! contract: reads registry handler::host_tool_descriptors
//! PRD-mcphost-event-kinds-registry-generated AC4: a tool embedding
//! `events::schema_enum()` that is called with `kind: "run.finished"` answers
//! `args_invalid` with `data.expected_one_of == ALL` and
//! `data.did_you_mean == "run.done"`. Also R2's drift scan: every host tool
//! schema carrying the event-kind enum embeds `schema_enum()` verbatim.

use mcphost::errors::AppError;
use mcphost::events::{self, EventKind};
use mcphost::kinds::KindRegistry;
use serde_json::{Value, json};

/// Every sub-schema (anywhere in `v`) whose `enum` array contains `run.done`.
fn event_enum_schemas<'a>(v: &'a Value, out: &mut Vec<&'a Value>) {
    match v {
        Value::Object(m) => {
            if m.get("enum").and_then(Value::as_array).is_some_and(|e| e.iter().any(|x| x == "run.done")) {
                out.push(v);
            }
            m.values().for_each(|c| event_enum_schemas(c, out));
        }
        Value::Array(a) => a.iter().for_each(|c| event_enum_schemas(c, out)),
        _ => {}
    }
}

#[test]
fn evkr_ac04() {
    let err = events::kind_arg(&json!({"kind": "run.finished"})).expect_err("run.finished is not a kind");
    let AppError::Structured { code, data, .. } = err else { panic!("expected a structured error") };
    assert_eq!(code, "args_invalid");
    let expected: Vec<&str> = EventKind::ALL.iter().map(|k| k.as_str()).collect();
    assert_eq!(data["expected_one_of"], json!(expected));
    assert_eq!(data["did_you_mean"], "run.done");

    // An unreachable spelling gets no suggestion rather than a wrong one.
    let AppError::Structured { data, .. } = events::parse_kind("zzz").unwrap_err() else { panic!() };
    assert_eq!(data["did_you_mean"], Value::Null);
    // A near typo is caught by edit distance.
    let AppError::Structured { data, .. } = events::parse_kind("run.dne").unwrap_err() else { panic!() };
    assert_eq!(data["did_you_mean"], "run.done");
    assert_eq!(events::parse_kind("run.done").unwrap(), EventKind::RunDone);

    // R2: every tool schema carrying the kind enum embeds schema_enum() exactly.
    let want = events::schema_enum();
    for tool in mcphost::handler::host_tool_descriptors(&KindRegistry::with_builtin()) {
        let schema = Value::Object((*tool.input_schema).clone());
        let mut hits = Vec::new();
        event_enum_schemas(&schema, &mut hits);
        for h in hits {
            assert_eq!(h["enum"], want["enum"], "tool {} spells the event kind enum by hand", tool.name);
        }
    }
}
