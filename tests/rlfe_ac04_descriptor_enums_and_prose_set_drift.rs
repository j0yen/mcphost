//! PRD-mcphost-runs-list-filter-enums-generated
//! AC4 (P0) -- Given `tools/list` for every `host.*` and `admin.*` tool, When
//! the drift test runs, Then `host.runs.list` and `admin.runs` carry
//! `status`/`trigger`/`verdict` enums equal to `RunStatus`/`RunTrigger`/
//! `Verdict` `names()` with description sets exactly `names().join("|")`; and
//! across ALL tools every prose set (R5 detectors D1/D2/D3) equals a property
//! `enum` or is one of the 5 named ratchet entries -- a new hit fails, a
//! listed entry that no longer hits fails, and a planted `(x|y)` fixture is
//! flagged.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::budget::Verdict;
use mcphost::runs::{RunStatus, RunTrigger};
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// Known gaps, owned by PRD-mcphost-list-filter-enums-generated-b. Each entry is
/// `(tool, property-or-set, detector)`; the list only shrinks.
const RATCHET: &[(&str, &str, &str)] = &[
    ("host.calls.list", "credential", "D2"),
    ("host.calls.list", "outcome", "D2"),
    ("host.table.describe", "key|category|measure|date|id|text", "D1"),
    ("host.oauth.grants", "cimd|dcr", "D1"),
    ("admin.triggers", "kind", "D3"),
];

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Hit {
    tool: String,
    key: String,
    detector: &'static str,
}

fn is_word(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// An identifier-shaped set member: starts with a letter (`1h, 24h` is a
/// duration list, not a name set).
fn is_ident(s: &str) -> bool {
    is_word(s) && s.starts_with(|c: char| c.is_ascii_alphabetic())
}

/// `(start-of-group, inner)` for every non-nested `( ... )` in `text`.
fn groups(text: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut open: Option<usize> = None;
    for (i, c) in text.char_indices() {
        match c {
            '(' => open = Some(i),
            ')' => {
                if let Some(o) = open.take() {
                    out.push((o, &text[o + 1..i]));
                }
            }
            _ => {}
        }
    }
    out
}

/// The word ending right before `idx` (skipping one space), if any.
fn word_before(text: &str, idx: usize) -> &str {
    let head = text[..idx].trim_end_matches(' ');
    let start = head.rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).map_or(0, |p| p + 1);
    &head[start..]
}

fn enum_sets(schema: &Value) -> Vec<BTreeSet<String>> {
    schema["properties"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(_, p)| p["enum"].as_array())
        .map(|e| e.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .collect()
}

fn enum_of(schema: &Value, prop: &str) -> Option<BTreeSet<String>> {
    schema["properties"][prop]["enum"]
        .as_array()
        .map(|e| e.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
}

/// D1 `(a|b|c)` and D2 `prop (a, b, c)` over one description text.
fn scan_text(tool: &str, text: &str, schema: &Value, hits: &mut Vec<Hit>) {
    let enums = enum_sets(schema);
    let props: BTreeSet<&str> = schema["properties"].as_object().into_iter().flatten().map(|(k, _)| k.as_str()).collect();
    for (start, inner) in groups(text) {
        let pipes: Vec<&str> = inner.split('|').collect();
        if pipes.len() >= 2 && pipes.iter().all(|p| is_word(p)) {
            let set: BTreeSet<String> = pipes.iter().map(|p| p.to_string()).collect();
            if !enums.contains(&set) {
                hits.push(Hit { tool: tool.into(), key: pipes.join("|"), detector: "D1" });
            }
            continue;
        }
        let prop = word_before(text, start);
        let commas: Vec<&str> = inner.split(',').map(str::trim).collect();
        if commas.len() >= 2 && props.contains(prop) && commas.iter().all(|p| is_ident(p) || *p == "...") {
            let set: BTreeSet<String> = commas.iter().map(|p| p.to_string()).collect();
            if enum_of(schema, prop).as_ref() != Some(&set) {
                hits.push(Hit { tool: tool.into(), key: prop.into(), detector: "D2" });
            }
        }
    }
}

/// D3: a `"type": "string"` property described `Only ... this <kind-word>`
/// with no `enum`.
fn scan_d3(tool: &str, schema: &Value, hits: &mut Vec<Hit>) {
    const WORDS: &[&str] = &["status", "kind", "state", "outcome", "verdict", "trigger kind", "method", "level"];
    for (name, prop) in schema["properties"].as_object().into_iter().flatten() {
        let desc = prop["description"].as_str().unwrap_or("");
        if prop["type"] != json!("string") || prop.get("enum").is_some() || !desc.starts_with("Only") {
            continue;
        }
        let Some(at) = desc.find("this ") else { continue };
        let rest = &desc[at + 5..];
        let hit = WORDS.iter().any(|w| {
            rest.strip_prefix(w).is_some_and(|tail| tail.chars().next().is_none_or(|c| !c.is_ascii_alphanumeric()))
        });
        if hit {
            hits.push(Hit { tool: tool.into(), key: name.clone(), detector: "D3" });
        }
    }
}

fn scan_tool(tool: &Value) -> Vec<Hit> {
    let name = tool["name"].as_str().expect("tool name");
    let schema = &tool["inputSchema"];
    let mut hits = Vec::new();
    scan_text(name, tool["description"].as_str().unwrap_or(""), schema, &mut hits);
    for prop in schema["properties"].as_object().into_iter().flatten().map(|(_, p)| p) {
        scan_text(name, prop["description"].as_str().unwrap_or(""), schema, &mut hits);
    }
    let mut d3 = Vec::new();
    scan_d3(name, schema, &mut d3);
    // A property already hit by D1/D2 is one finding, not two.
    let covered: BTreeSet<String> = hits.iter().map(|h| h.key.clone()).collect();
    hits.extend(d3.into_iter().filter(|h| !covered.contains(&h.key)));
    hits.sort();
    hits.dedup();
    hits
}

fn tools_of(listed: &Value, prefix: &str) -> Vec<Value> {
    listed["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .filter(|t| t["name"].as_str().is_some_and(|n| n.starts_with(prefix)))
        .cloned()
        .collect()
}

fn names_set(names: Vec<&str>) -> BTreeSet<String> {
    names.into_iter().map(str::to_string).collect()
}

fn assert_field(tool: &Value, prop: &str, names: Vec<&'static str>) {
    let tname = tool["name"].as_str().unwrap();
    let schema = &tool["inputSchema"];
    assert_eq!(enum_of(schema, prop), Some(names_set(names.clone())), "{tname}.{prop} enum != names()");
    let set = names.join("|");
    let ordered: Vec<&str> = schema["properties"][prop]["enum"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert_eq!(ordered, names, "{tname}.{prop} enum order");
    let pdesc = schema["properties"][prop]["description"].as_str().unwrap();
    assert!(pdesc.contains(&format!("({set})")), "{tname}.{prop} description lacks ({set}): {pdesc}");
    let tdesc = tool["description"].as_str().unwrap();
    assert!(tdesc.contains(&format!("({set}")), "{tname} description lacks the {set} set: {tdesc}");
}

#[tokio::test]
async fn rlfe_ac04() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "RLFE AC4 Tenant").await;
    let tenant = McpClient::with_bearer(&server.base_url, &key);
    let admin = McpClient::with_bearer(&server.base_url, common::ADMIN_KEY);
    let mut tools = tools_of(&tenant.tools_list().await.expect("tenant tools/list"), "host.");
    tools.extend(tools_of(&admin.tools_list().await.expect("admin tools/list"), "admin."));
    assert!(tools.len() > 100, "tools/list enumeration looks broken: {}", tools.len());

    // (a) per-field: enums and description sets equal names().
    let by_name = |n: &str| tools.iter().find(|t| t["name"] == json!(n)).unwrap_or_else(|| panic!("{n} not listed"));
    let list = by_name("host.runs.list");
    assert_field(list, "status", RunStatus::names());
    assert_field(list, "trigger", RunTrigger::names());
    assert_field(list, "verdict", Verdict::names());
    assert_field(by_name("admin.runs"), "status", RunStatus::names());
    let docs = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/tools.md")).expect("docs/tools.md");
    for set in [RunStatus::names(), RunTrigger::names(), Verdict::names()] {
        // The tools table escapes the pipe so it does not split a cell.
        assert!(docs.contains(&set.join("\\|")), "docs/tools.md lacks {}", set.join("|"));
    }

    // (b) class-level: every prose set equals a property enum or is ratcheted.
    let found: BTreeSet<Hit> = tools.iter().flat_map(scan_tool).collect();
    let ratchet: BTreeSet<Hit> = RATCHET
        .iter()
        .map(|(t, k, d)| Hit { tool: t.to_string(), key: k.to_string(), detector: d })
        .collect();
    let new: Vec<_> = found.difference(&ratchet).collect();
    assert!(new.is_empty(), "prose set without a matching enum (new hit): {new:#?}");
    let stale: Vec<_> = ratchet.difference(&found).collect();
    assert!(stale.is_empty(), "ratchet entry that no longer hits (remove it): {stale:#?}");
}

#[test]
fn rlfe_ac04_planted_fixture_is_flagged() {
    let d1 = json!({"name": "fixture.pipe", "description": "List things of kind (x|y).",
        "inputSchema": {"properties": {"kind": {"type": "string"}}}});
    assert_eq!(scan_tool(&d1), vec![Hit { tool: "fixture.pipe".into(), key: "x|y".into(), detector: "D1" }]);
    let d2 = json!({"name": "fixture.comma", "description": "Filter by outcome (ok, error, refused).",
        "inputSchema": {"properties": {"outcome": {"type": "string"}}}});
    assert_eq!(scan_tool(&d2).len(), 1);
    let d3 = json!({"name": "fixture.only", "description": "List.",
        "inputSchema": {"properties": {"state": {"type": "string", "description": "Only rows in this state."}}}});
    assert_eq!(scan_tool(&d3).len(), 1);
    let ok = json!({"name": "fixture.ok", "description": "List by kind (x|y).",
        "inputSchema": {"properties": {"kind": {"type": "string", "enum": ["x", "y"]}}}});
    assert!(scan_tool(&ok).is_empty());
}
