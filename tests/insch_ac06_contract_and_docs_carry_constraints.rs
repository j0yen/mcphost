//! PRD-mcphost-input-schema-is-the-validator
//! AC6 (P1) -- Given the contract dump and `docs/tools.md`, When regenerated,
//! Then every `anyOf`, `enum`, `pattern`, `minimum`, `maximum`, `minItems` and
//! `maxItems` from the descriptors appears under `input_schema` and in the
//! rendered argument lists, with no drift excuse added.

use mcphost::handler::{canonical_control_plane_names, host_tool_descriptors};
use mcphost::kinds::KindRegistry;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::Path;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const KEYWORDS: &[&str] = &["anyOf", "enum", "pattern", "minimum", "maximum", "minItems", "maxItems"];

/// Every (json pointer, keyword, value) among the constraint keywords, at any
/// depth of a schema.
fn constraints(node: &Value, at: &str, out: &mut Vec<(String, String, Value)>) {
    match node {
        Value::Object(map) => {
            for (k, v) in map {
                let here = format!("{at}/{k}");
                // A property literally named like a keyword is not a constraint.
                let is_props = at.ends_with("/properties");
                if !is_props && KEYWORDS.contains(&k.as_str()) {
                    out.push((at.to_string(), k.clone(), v.clone()));
                }
                constraints(v, &here, out);
            }
        }
        Value::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                constraints(v, &format!("{at}/{i}"), out);
            }
        }
        _ => {}
    }
}

fn contract() -> BTreeMap<String, Value> {
    let doc: Value =
        serde_json::from_slice(&std::fs::read(Path::new(ROOT).join("contracts/host-tools.v1.json")).expect("contract")).expect("json");
    doc["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|t| (t["name"].as_str().expect("name").to_string(), t.clone()))
        .collect()
}

#[test]
fn the_contract_dump_carries_every_descriptor_constraint_under_input_schema() {
    let tools = host_tool_descriptors(&KindRegistry::with_builtin());
    let contract = contract();
    let mut problems = Vec::new();
    let mut total = 0;
    let mut with_any_of = 0;
    for tool in &tools {
        let name = tool.name.to_string();
        let schema = Value::Object((*tool.input_schema).clone());
        let mut want = Vec::new();
        constraints(&schema, "", &mut want);
        total += want.len();
        if tool.input_schema.contains_key("anyOf") && name.contains('.') {
            with_any_of += 1;
        }
        let Some(entry) = contract.get(&name) else {
            problems.push(format!("{name}: not in the contract"));
            continue;
        };
        let mut have = Vec::new();
        constraints(&entry["input_schema"], "", &mut have);
        for w in &want {
            if !have.contains(w) {
                problems.push(format!("{name}: {} {} missing under input_schema", w.0, w.1));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
    assert!(total >= 100, "only {total} constraints seen");
    // Success metric: descriptors with `anyOf` >= 12.
    assert!(with_any_of >= 12, "only {with_any_of} canonical descriptors carry anyOf");
}

/// The text of `tool`'s `**Arguments**` block.
fn arguments_block(doc: &str, tool: &str) -> String {
    let start = doc.find(&format!("### `{tool}`\n")).unwrap_or_else(|| panic!("no section for {tool}"));
    let rest = &doc[start..];
    let end = rest[4..].find("\n### ").map_or(rest.len(), |i| i + 4);
    let section = &rest[..end];
    let args = section.find("**Arguments**").unwrap_or_else(|| panic!("{tool}: no Arguments block"));
    section[args..].to_string()
}

#[test]
fn tools_doc_argument_lists_carry_every_descriptor_constraint() {
    let kinds = KindRegistry::with_builtin();
    let committed = std::fs::read_to_string(Path::new(ROOT).join("docs/tools.md")).expect("docs/tools.md");
    assert_eq!(mcphost::gendocs::render_tools_markdown(&kinds), committed, "docs/tools.md is stale: run `mcphost gen-docs`");
    let tools: BTreeMap<String, Map<String, Value>> =
        host_tool_descriptors(&kinds).into_iter().map(|t| (t.name.to_string(), (*t.input_schema).clone())).collect();
    let mut problems = Vec::new();
    let mut seen = 0;
    for name in canonical_control_plane_names(&kinds) {
        let block = arguments_block(&committed, &name);
        let mut found = Vec::new();
        constraints(&Value::Object(tools[&name].clone()), "", &mut found);
        for (at, keyword, value) in found {
            seen += 1;
            let fragments: Vec<String> = match keyword.as_str() {
                "enum" => value.as_array().expect("enum array").iter().map(|m| match m {
                    Value::String(s) => format!("`{s}`"),
                    other => format!("`{other}`"),
                }).collect(),
                "pattern" => vec![format!("`{}`", value.as_str().expect("pattern"))],
                "anyOf" => value
                    .as_array()
                    .expect("anyOf array")
                    .iter()
                    .flat_map(|b| b["required"].as_array().into_iter().flatten().filter_map(Value::as_str).map(|a| format!("`{a}`")).collect::<Vec<_>>())
                    .collect(),
                other => vec![format!("{other}: {value}")],
            };
            for f in fragments {
                if !block.contains(&f) {
                    problems.push(format!("{name}: `{at}` {keyword} fragment {f} missing from the Arguments block"));
                }
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
    assert!(seen >= 100, "only {seen} constraints seen");
}

#[test]
fn no_deprecation_excuse_was_added_for_the_new_constraints() {
    let ledger: Value =
        serde_json::from_slice(&std::fs::read(Path::new(ROOT).join("contracts/deprecations.json")).expect("ledger")).expect("json");
    let paths: Vec<&str> = ledger.as_array().expect("array").iter().filter_map(|d| d["path"].as_str()).collect();
    for path in [
        "host.docs.get",
        "host.trigger.get",
        "host.notify.send.level",
        "host.table.model_set.key",
        "host.policy.set.target",
        "host.index.create.name",
        "host.index.create.sources",
        "host.task.post.deadline_s",
        "host.ask.create.options",
        "host.table.append.rows",
        "host.policy.attrs_set.attrs",
        "host.table.create.name",
    ] {
        assert!(!paths.contains(&path), "{path} has a deprecations entry: the constraints are additive");
    }
}
