//! PRD-mcphost-schema-shape-agreement
//! contract: reads registry handler::host_tool_descriptors
//! AC7 (P0) -- Given the PR head, When `mcphost contract dump --check` and the
//! `docs/tools.md` drift check run, Then `contracts/host-tools.v1.json` carries
//! every new keyword, both pass with no diff, and
//! `planlim_ac07_contract_drift_with_deprecation_entry` passes, with one
//! deprecation entry per narrowed descriptor if it requires one.

use mcphost::api_contract::{Violation, diff, dump_contract, dump_contract_bytes, load_deprecations};
use mcphost::kinds::KindRegistry;
use serde_json::Value;
use std::path::Path;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

/// Every keyword this PRD added to a descriptor: `(tool, JSON pointer into
/// input_schema, keyword)`. Each also exists under the flattened `host_...` name.
const ADDED: &[(&str, &str, &str)] = &[
    ("host.agent.profile_set", "/properties/handle", "pattern"),
    ("host.ask.create", "/properties/options", "uniqueItems"),
    ("host.ask.create", "/properties/options/items", "pattern"),
    ("host.ask.create", "/properties/ttl_s", "maximum"),
    ("host.ask.create", "/properties/ttl_s", "minimum"),
    ("host.blob.put", "/properties/content_base64", "pattern"),
    ("host.claim.dns.check", "/properties/domain", "pattern"),
    ("host.claim.dns.start", "/properties/domain", "pattern"),
    ("host.email.send", "/properties/reply_to_message_id", "pattern"),
    ("host.email.send", "/properties/to/oneOf/0", "pattern"),
    ("host.email.send", "/properties/to/oneOf/1/items", "pattern"),
    ("host.lineage.blast_radius", "/properties/change_kind", "enum"),
    ("host.notify.send", "/properties/link", "pattern"),
    ("host.oauth.scope_set", "/properties/name", "pattern"),
    ("host.policy.set", "/properties/rule", "type"),
    ("host.policy.set", "/properties/rule/items/properties/op", "enum"),
    ("host.secret.leak_test", "/properties/name", "pattern"),
    ("host.secret.set", "/properties/hosts/items", "pattern"),
    ("host.sync.create", "/properties/primary_key", "pattern"),
    ("host.sync.create", "/properties/table", "pattern"),
    ("host.tool.spec_shared", "/properties/tool", "pattern"),
    ("host.uptime.create", "/properties/name", "pattern"),
    ("host.webhook_inbox.create", "/properties/name", "pattern"),
    ("host.docs.index_config", "/properties/provider", "enum"),
    ("host.docs.put", "/properties/mime", "enum"),
    ("host.docs.search", "/properties/mode", "enum"),
    ("host.calls.list", "/properties/credential", "enum"),
    ("host.usage", "/properties/by", "enum"),
    ("billing.checkout", "/properties/source", "enum"),
];

/// The subset of [`ADDED`] the comparator reads as narrowing (an enum where none
/// existed, a tightened maximum); the others it reports as nothing.
const NARROWING: &[&str] = &["enum", "maximum"];

fn flat(name: &str) -> String {
    name.replace('.', "_")
}

fn contract() -> Value {
    serde_json::from_slice(&std::fs::read(Path::new(ROOT).join("contracts/host-tools.v1.json")).expect("contract")).expect("json")
}

fn names_for(doc: &Value, tool: &str) -> Vec<String> {
    [tool.to_string(), flat(tool)]
        .into_iter()
        .filter(|n| doc["tools"].as_array().unwrap().iter().any(|t| t["name"] == n.as_str()))
        .collect()
}

#[test]
fn shp_ac07_contract_carries_every_new_shape_keyword() {
    let doc = contract();
    for (tool, pointer, keyword) in ADDED {
        let names = names_for(&doc, tool);
        assert!(names.contains(&tool.to_string()), "{tool} not in the contract");
        for name in names {
            let t = doc["tools"].as_array().unwrap().iter().find(|t| t["name"] == name.as_str()).unwrap();
            let at = t["input_schema"].pointer(pointer).unwrap_or_else(|| panic!("{name} {pointer}"));
            assert!(at.get(*keyword).is_some(), "{name} {pointer} carries no {keyword}: {at}");
        }
    }
}

#[test]
fn shp_ac07_contract_dump_check_and_tools_doc_have_no_diff() {
    let kinds = KindRegistry::with_builtin();
    let committed = std::fs::read(Path::new(ROOT).join("contracts/host-tools.v1.json")).expect("contract");
    assert_eq!(dump_contract_bytes(&kinds), committed, "contracts/host-tools.v1.json is stale: run `mcphost contract dump`");
    let doc = std::fs::read_to_string(Path::new(ROOT).join("docs/tools.md")).expect("docs/tools.md");
    assert_eq!(mcphost::gendocs::render_tools_markdown(&kinds), doc, "docs/tools.md is stale: run `mcphost gen-docs`");
}

#[test]
fn shp_ac07_each_narrowed_descriptor_has_a_deprecation_entry() {
    let live = dump_contract(&KindRegistry::with_builtin());
    let mut before = contract();
    for (tool, pointer, keyword) in ADDED.iter().filter(|(_, _, k)| NARROWING.contains(k)) {
        for name in names_for(&before.clone(), tool) {
            let t = before["tools"].as_array_mut().unwrap().iter_mut().find(|t| t["name"] == name.as_str()).unwrap();
            if let Some(obj) = t["input_schema"].pointer_mut(pointer).and_then(Value::as_object_mut) {
                obj.remove(*keyword);
            }
        }
    }
    let ledger = load_deprecations(&Path::new(ROOT).join("contracts/deprecations.json")).expect("deprecations.json");
    let unexcused = diff(&before, &live, &[], mcphost::state::now_unix());
    assert!(!unexcused.is_empty(), "the new enums must read as narrowing without the ledger");
    for v in &unexcused {
        assert!(matches!(v, Violation::Narrowed { .. }), "{v:?}");
        assert!(
            ledger.iter().any(|d| d.narrowing_only && d.lead_time_valid() && d.path == v.path() && d.replacement.contains("PRD-mcphost-schema-shape-agreement")),
            "no narrowing_only deprecation entry names {}",
            v.path()
        );
    }
    let excused = diff(&before, &live, &ledger, mcphost::state::now_unix());
    assert!(excused.is_empty(), "with the ledger the drift check must pass: {excused:?}");
}
