//! contract: reads registry handler::host_tool_descriptors
//! contract: reads contracts/host-output-observational-edits.v1.json
//! PRD-mcphost-success-result-matches-output-schema
//! AC4 (P0) -- Given `contracts/host-output-observational-edits.v1.json`, When
//! the edits test loads it, Then every entry names a tool `tools/list` serves,
//! a JSON pointer that resolves in that tool's schema at the cited tree, an
//! edit kind of `drop_required`, `add_null` or `empty`, and an emitter
//! file:line that exists, and the entry count is at most the committed
//! `max_entries`.

use std::collections::BTreeSet;
use std::path::Path;

use mcphost::handler::host_tool_descriptors;
use mcphost::kinds::KindRegistry;
use serde_json::Value;

fn raw_file() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("contracts/host-output-observational-edits.v1.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("edits file")).expect("edits file is JSON")
}

/// The output schema of every canonical tool at the cited tree.
fn cited(tool: &str) -> Option<Value> {
    let raw: Value = serde_json::from_str(include_str!("fixtures/osmatch/cited-output-schemas.json")).expect("fixture");
    raw["tools"]
        .as_array()?
        .iter()
        .find(|t| t["name"] == tool)
        .map(|t| t["output_schema"].clone())
}

#[test]
fn every_entry_names_a_served_tool_a_resolving_pointer_a_known_kind_and_a_real_emitter_line() {
    let file = raw_file();
    let entries = file["entries"].as_array().expect("entries");
    let served: BTreeSet<String> = host_tool_descriptors(&KindRegistry::with_builtin())
        .iter()
        .map(|t| t.name.to_string())
        .collect();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut problems = Vec::new();
    for entry in entries {
        let tool = entry["tool"].as_str().unwrap_or_default();
        let pointer = entry["pointer"].as_str().unwrap_or("<missing>");
        let kind = entry["edit"].as_str().unwrap_or_default();
        let at = format!("{tool} {pointer}");
        if !served.contains(tool) {
            problems.push(format!("{at}: tools/list does not serve {tool}"));
        }
        match cited(tool) {
            None => problems.push(format!("{at}: no cited-tree schema for {tool}")),
            Some(schema) => {
                if schema.pointer(pointer).is_none() {
                    problems.push(format!("{at}: pointer does not resolve at the cited tree"));
                } else if kind == "drop_required" {
                    let (parent, name) = pointer.rsplit_once("/properties/").unwrap_or(("", ""));
                    let required = schema.pointer(parent).and_then(|p| p.get("required")).and_then(Value::as_array);
                    if !required.is_some_and(|r| r.iter().any(|v| v == name)) {
                        problems.push(format!("{at}: `{name}` is not required at the cited tree"));
                    }
                }
            }
        }
        if !["drop_required", "add_null", "empty"].contains(&kind) {
            problems.push(format!("{at}: unknown edit kind {kind:?}"));
        }
        if kind == "empty" && !pointer.is_empty() {
            problems.push(format!("{at}: an `empty` edit applies to the whole result, its pointer is \"\""));
        }
        let emitter = entry["emitter"].as_str().unwrap_or_default();
        match emitter.rsplit_once(':').and_then(|(f, l)| Some((f, l.parse::<usize>().ok()?))) {
            Some((file, line)) => match std::fs::read_to_string(root.join(file)) {
                Ok(src) if line >= 1 && line <= src.lines().count() => {}
                Ok(_) => problems.push(format!("{at}: emitter {emitter} is past the end of the file")),
                Err(e) => problems.push(format!("{at}: emitter file {file}: {e}")),
            },
            None => problems.push(format!("{at}: emitter {emitter:?} is not <file>:<line>")),
        }
    }
    assert!(!entries.is_empty(), "the file carries the initial entries");
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn the_entry_count_never_exceeds_the_committed_max_entries() {
    let file = raw_file();
    let count = file["entries"].as_array().expect("entries").len();
    let max = file["max_entries"].as_u64().expect("max_entries") as usize;
    assert!(count <= max, "{count} entries exceed the committed max_entries {max}; the count can only shrink");
    assert_eq!(mcphost::output_schemas::observational_edits().entries.len(), count, "the registry loads every entry");
    let mut seen = BTreeSet::new();
    for e in &mcphost::output_schemas::observational_edits().entries {
        assert!(seen.insert((e.tool.clone(), e.pointer.clone(), format!("{:?}", e.edit))), "duplicate entry {} {} {:?}", e.tool, e.pointer, e.edit);
    }
}
