//! PRD-mcphost-input-schema-is-the-validator
//! contract: every `missing required argument` literal in src/ names an argument in the owning host tool's required or anyOf
//! AC1 (P0) -- Given the fourteen tools named in the problem statement, When
//! `tools/list` is read by an authenticated caller, Then each either-or tool
//! carries `anyOf` with one `required` branch per alternative and each
//! plain-required tool lists the argument in `required`, and a scan of every
//! `missing required argument` literal in `src/` finds its argument in the
//! owning tool's `required` or `anyOf`.

use mcphost::handler::host_tool_descriptors;
use mcphost::kinds::KindRegistry;
use serde_json::Value;
use std::collections::BTreeSet;

/// (tool, alternatives): the either-or tools, alternatives exactly as the
/// handler's refusal names them.
const EITHER_OR: &[(&str, &[&str])] = &[
    ("host.docs.get", &["id", "name"]),
    ("host.docs.delete", &["id", "name"]),
    ("host.docs.purge", &["id", "name"]),
    ("host.blob.get", &["id", "name"]),
    ("host.trigger.get", &["id", "name"]),
    ("host.trigger.fire", &["id", "name"]),
    ("host.trigger.pause", &["id", "name"]),
    ("host.trigger.remove", &["id", "name"]),
    ("host.trigger.resume", &["id", "name"]),
    ("host.trigger.test", &["id", "name"]),
    ("host.docs.put", &["content", "content_base64"]),
    ("host.blob.put", &["content_base64", "url"]),
    // `name` or `group` (a group channel): `name` is the argument the
    // handler's refusal names, `group` the alternative its description gives.
    ("host.channel.open", &["name", "group"]),
    // `run_id`, or a dead letter id, or a webhook inbox `row_id`.
    ("host.trigger.replay", &["run_id", "dead_letter_id", "row_id"]),
];

fn tools() -> Vec<rmcp::model::Tool> {
    host_tool_descriptors(&KindRegistry::with_builtin())
}

/// Every argument a schema names in `required` or in an `anyOf` branch's
/// `required`.
fn named_args(schema: &serde_json::Map<String, Value>) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut take = |v: Option<&Value>| {
        for a in v.and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
            out.insert(a.to_string());
        }
    };
    take(schema.get("required"));
    for branch in schema.get("anyOf").and_then(Value::as_array).into_iter().flatten() {
        take(branch.get("required"));
    }
    // A conditional requirement: `allOf: [{if, then: {required}}]`.
    for cond in schema.get("allOf").and_then(Value::as_array).into_iter().flatten() {
        take(cond.get("then").and_then(|t| t.get("required")));
    }
    out
}

#[test]
fn either_or_tools_carry_one_required_branch_per_alternative() {
    let tools = tools();
    let mut problems = Vec::new();
    for (name, alts) in EITHER_OR {
        let Some(tool) = tools.iter().find(|t| t.name.as_ref() == *name) else {
            problems.push(format!("{name}: not listed"));
            continue;
        };
        let branches: Vec<Vec<String>> = tool
            .input_schema
            .get("anyOf")
            .and_then(Value::as_array)
            .map(|b| {
                b.iter()
                    .map(|branch| {
                        branch["required"]
                            .as_array()
                            .map(|r| r.iter().filter_map(Value::as_str).map(str::to_string).collect())
                            .unwrap_or_default()
                    })
                    .collect()
            })
            .unwrap_or_default();
        let want: Vec<Vec<String>> = alts.iter().map(|a| vec![a.to_string()]).collect();
        if branches != want {
            problems.push(format!("{name}: anyOf branches {branches:?}, want {want:?}"));
        }
        let props = tool.input_schema["properties"].as_object().expect("properties");
        for a in *alts {
            if !props.contains_key(*a) {
                problems.push(format!("{name}: anyOf names `{a}` which is not a property"));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn name_is_listed_in_required_on_the_plain_required_tools() {
    let tools = tools();
    for name in ["host.docs.put", "host.blob.put"] {
        let tool = tools.iter().find(|t| t.name.as_ref() == name).expect(name);
        let required = named_args(&tool.input_schema);
        assert!(required.contains("name"), "{name}: `name` missing from required: {required:?}");
        let plain: Vec<&str> = tool.input_schema["required"]
            .as_array()
            .expect("required")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(plain.contains(&"name"), "{name}: `name` not in plain required {plain:?}");
    }
    let purge = tools.iter().find(|t| t.name.as_ref() == "host.docs.purge").expect("purge");
    assert!(named_args(&purge.input_schema).contains("older_than_versions"));
}

/// The `host.*` tool prefixes each source file's handlers serve; a file not
/// listed is checked against every host tool.
fn owning_prefixes(file: &str) -> Option<&'static [&'static str]> {
    match file {
        "docs.rs" => Some(&["host.docs."]),
        "blobs.rs" => Some(&["host.blob."]),
        "triggers.rs" => Some(&["host.trigger.", "host.schedule"]),
        "channels.rs" => Some(&["host.channel."]),
        "index.rs" => Some(&["host.index."]),
        "asks.rs" => Some(&["host.ask."]),
        "tables.rs" => Some(&["host.table."]),
        "uptime.rs" => Some(&["host.uptime."]),
        "webhook_inbox.rs" => Some(&["host.webhook_inbox."]),
        "rowpolicy/mod.rs" => Some(&["host.policy."]),
        "notes.rs" => Some(&["host.note"]),
        _ => None,
    }
}

fn literal_args(line: &str) -> Vec<String> {
    let Some(at) = line.find("missing required argument") else { return Vec::new() };
    let rest = &line[at + "missing required argument".len()..];
    let end = rest.find('"').unwrap_or(rest.len());
    let rest = &rest[..end];
    let mut out = Vec::new();
    let mut chars = rest.split('\'');
    chars.next();
    for (i, seg) in chars.enumerate() {
        // Odd segments sit between quote pairs.
        if i % 2 == 0 && !seg.is_empty() && seg.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
            out.push(seg.to_string());
        }
    }
    out
}

fn rs_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read_dir") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            rs_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn every_missing_required_argument_literal_is_in_the_owning_tools_schema() {
    let tools = tools();
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rs_files(&src, &mut files);
    let mut problems = Vec::new();
    let mut checked = 0;
    for file in files {
        let rel = file.strip_prefix(&src).expect("under src").to_string_lossy().to_string();
        // admin.* tools are not in `host_tools`; handler.rs/others route by
        // name, so they fall back to "some host tool names it".
        if rel == "admin.rs" {
            continue;
        }
        // `ok` is `admin.example_proof_report`'s, an admin tool.
        let admin_only = |rel: &str, arg: &str| rel == "example_proofs.rs" && arg == "ok";
        let text = std::fs::read_to_string(&file).expect("read");
        for (n, line) in text.lines().enumerate() {
            for arg in literal_args(line) {
                if admin_only(&rel, &arg) {
                    continue;
                }
                checked += 1;
                let prefixes = owning_prefixes(&rel);
                let owners: Vec<&rmcp::model::Tool> = tools
                    .iter()
                    .filter(|t| prefixes.is_none_or(|ps| ps.iter().any(|p| t.name.starts_with(p))))
                    .collect();
                if !owners.iter().any(|t| named_args(&t.input_schema).contains(&arg)) {
                    problems.push(format!("{rel}:{}: `{arg}` is in no owning tool's required/anyOf", n + 1));
                }
            }
        }
    }
    assert!(checked >= 40, "scan found only {checked} literals");
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
