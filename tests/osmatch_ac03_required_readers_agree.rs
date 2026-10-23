//! contract: reads registry handler::host_tool_descriptors
//! contract: reads contracts/host-output-observational-edits.v1.json
//! PRD-mcphost-success-result-matches-output-schema
//! AC3 (P0) -- Given every tool in the composed registry, When the drift test
//! reads required keys through gendocs' Returns block, through
//! `api_contract::diff_output_properties` and through
//! `output_schemas::live_required`, Then all three agree for every tool, a tool
//! with no variant and no entry is byte-identical to the cited tree, and a tool
//! with variants keeps root `type: object` and its live `properties`.

use std::collections::{BTreeMap, BTreeSet};

use mcphost::api_contract::{Violation, diff};
use mcphost::kinds::KindRegistry;
use mcphost::output_schemas::{live_required, observational_edits, output_schema_for, registered_tools};
use serde_json::{Map, Value, json};

/// The registry's output schemas at the cited tree (`contracts/host-tools.v1.json`'s
/// `output_schema` of every canonical tool, before this PRD).
fn cited() -> BTreeMap<String, Value> {
    let raw: Value = serde_json::from_str(include_str!("fixtures/osmatch/cited-output-schemas.json")).expect("fixture");
    raw["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|t| (t["name"].as_str().expect("name").to_string(), t["output_schema"].clone()))
        .collect()
}

fn contract_of(tool: &str, schema: &Value) -> Value {
    json!({"tools": [{"name": tool, "input_schema": {}, "output_schema": schema}]})
}

/// `tool -> {field}` marked `(always)` in gendocs' Returns block.
fn docs_required() -> BTreeMap<String, BTreeSet<String>> {
    let doc = mcphost::gendocs::render_tools_markdown(&KindRegistry::with_builtin());
    let mut out = BTreeMap::new();
    let mut tool: Option<String> = None;
    let mut in_returns = false;
    for line in doc.lines() {
        if let Some(name) = line.strip_prefix("### `").and_then(|l| l.strip_suffix('`')) {
            tool = Some(name.to_string());
            in_returns = false;
            out.entry(name.to_string()).or_insert_with(BTreeSet::new);
        } else if line == "**Returns**" {
            in_returns = true;
        } else if line == "**Arguments**" {
            in_returns = false;
        } else if in_returns
            && let Some(tool) = &tool
            && let Some(rest) = line.strip_prefix("- `")
            && rest.ends_with(" (always)")
        {
            let field = rest.split_once('`').map(|(f, _)| f).unwrap_or(rest);
            out.get_mut(tool).expect("tool entered").insert(field.to_string());
        }
    }
    out
}

#[test]
fn gendocs_diff_and_live_required_agree_for_every_tool() {
    let docs = docs_required();
    let tools = registered_tools();
    assert!(tools.len() > 200, "{} tools", tools.len());
    let mut problems = Vec::new();
    for tool in tools {
        let schema = output_schema_for(tool).expect("registered");
        let live: BTreeSet<String> = live_required(&schema).into_iter().map(str::to_string).collect();
        let props: Map<String, Value> = schema.get("properties").and_then(Value::as_object).cloned().unwrap_or_default();
        let rendered = docs.get(tool).cloned().unwrap_or_default();
        let live_in_props: BTreeSet<String> = live.iter().filter(|k| props.contains_key(*k)).cloned().collect();
        if rendered != live_in_props {
            problems.push(format!("{tool}: gendocs renders {rendered:?}, live_required is {live_in_props:?}"));
        }
        // diff: an old schema that requires exactly the live keys is not
        // narrowed by the registry schema ...
        let schema_v = Value::Object((*schema).clone());
        let mut flat = json!({"type": "object", "properties": props});
        if !live.is_empty() {
            flat["required"] = json!(live);
        }
        let findings = diff(&contract_of(tool, &flat), &contract_of(tool, &schema_v), &[], 0);
        if !findings.is_empty() {
            problems.push(format!("{tool}: diff reads required differently from live_required: {findings:?}"));
        }
        // ... and a key the registry does not require is reported.
        if let Some(extra) = props.keys().find(|k| !live.contains(*k)) {
            let mut stricter = flat.clone();
            stricter["required"] = json!(live.iter().cloned().chain([extra.clone()]).collect::<Vec<_>>());
            let findings = diff(&contract_of(tool, &stricter), &contract_of(tool, &schema_v), &[], 0);
            let named = findings.iter().any(
                |f| matches!(f, Violation::Narrowed { path, detail } if path == &format!("{tool}.output.{extra}") && detail.contains("no longer always present")),
            );
            if !named {
                problems.push(format!("{tool}: dropping required `{extra}` was not reported: {findings:?}"));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn live_required_is_the_root_required_of_the_cited_tree_for_every_tool() {
    let mut problems = Vec::new();
    for (tool, old) in cited() {
        let Some(new) = output_schema_for(&tool) else {
            problems.push(format!("{tool}: no longer registered"));
            continue;
        };
        let want: BTreeSet<&str> = old
            .get("required")
            .and_then(Value::as_array)
            .map(|r| r.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let got: BTreeSet<&str> = live_required(&new).into_iter().collect();
        // An observational `drop_required` entry is the one reviewed exception.
        let dropped: BTreeSet<&str> = observational_edits()
            .entries
            .iter()
            .filter(|e| e.tool == tool && e.edit == mcphost::output_schemas::EditKind::DropRequired)
            .filter_map(|e| e.pointer.rsplit('/').next())
            .collect();
        let want: BTreeSet<&str> = want.difference(&dropped).copied().collect();
        if want != got {
            problems.push(format!("{tool}: cited required {want:?}, live_required {got:?}"));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn a_tool_without_variant_or_entry_is_byte_identical_and_a_variant_tool_keeps_its_root() {
    let mut edited: BTreeSet<&str> = observational_edits().entries.iter().map(|e| e.tool.as_str()).collect();
    // PRD-mcphost-output-schema-reach-sweep-live R5: `host.doctor` gained the
    // optional `output_reach` block after the cited tree.
    edited.insert("host.doctor");
    // A tenant-defined value the corpus can only sample is declared free-form
    // (`output_schemas::OPAQUE_FIELDS`): `host.table.query` rows are the
    // tenant's columns, and `host.tool.call` merges a tenant tool's payload
    // (its `id`, `rows`) into the root.
    edited.insert("host.table.query");
    edited.insert("host.tool.call");
    let (mut untouched, mut varianted) = (0, 0);
    let mut problems = Vec::new();
    for (tool, old) in cited() {
        let new = Value::Object((*output_schema_for(&tool).expect("registered")).clone());
        let has_variant = new.get("anyOf").is_some();
        if !has_variant && !edited.contains(tool.as_str()) {
            untouched += 1;
            if serde_json::to_string(&old).expect("ser") != serde_json::to_string(&new).expect("ser") {
                problems.push(format!("{tool}: not byte-identical to the cited tree"));
            }
        }
        if has_variant {
            varianted += 1;
            if new.get("type") != Some(&json!("object")) {
                problems.push(format!("{tool}: variants lost the root `type: object`"));
            }
            // The five `would` previews moved into the dry-run branch.
            let keys = |v: &Value| -> BTreeSet<String> {
                v.get("properties").and_then(Value::as_object).map(|p| p.keys().cloned().collect()).unwrap_or_default()
            };
            let (old_keys, new_keys) = (keys(&old), keys(&new));
            let lost: Vec<_> = old_keys.difference(&new_keys).filter(|k| k.as_str() != "would").collect();
            if !lost.is_empty() || !new_keys.is_subset(&old_keys) {
                problems.push(format!("{tool}: live properties changed: lost {lost:?}"));
            }
            let first = new["anyOf"][0].get("type");
            if first != Some(&json!("object")) {
                problems.push(format!("{tool}: first anyOf branch is not the live object branch"));
            }
            if new.get("required").is_some() {
                problems.push(format!("{tool}: root `required` should have moved into the live branch"));
            }
        }
    }
    assert!(untouched > 150, "only {untouched} untouched tools compared");
    assert!(varianted >= 44, "only {varianted} tools carry variants");
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
