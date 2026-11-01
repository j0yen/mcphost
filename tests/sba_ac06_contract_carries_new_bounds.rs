//! PRD-mcphost-schema-bound-agreement
//! contract: reads registry handler::host_tool_descriptors
//! AC6 (P0) -- Given the PR head, When `mcphost contract dump --check` and the
//! `docs/tools.md` drift check run, Then `contracts/host-tools.v1.json` carries
//! every new bound keyword, both pass with no diff, and
//! `planlim_ac07_contract_drift_with_deprecation_entry` passes, with one
//! deprecation entry per narrowed descriptor if it requires one.

use mcphost::api_contract::{Violation, diff, dump_contract, dump_contract_bytes, load_deprecations};
use mcphost::kinds::KindRegistry;
use serde_json::Value;
use std::path::Path;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

/// Every bound keyword this PRD added to a descriptor: `(tool, JSON pointer into
/// input_schema, keyword)`. Each also exists under the flattened `host_...` name.
const ADDED: &[(&str, &str, &str)] = &[
    ("host.agent.contacts_import", "/properties/addresses", "maxItems"),
    ("host.agent.contacts_import", "/properties/addresses", "minItems"),
    ("host.agent.profile_set", "/properties/tags", "maxItems"),
    ("host.ask.create", "/properties/question", "minLength"),
    ("host.audit.export", "/properties/limit", "maximum"),
    ("host.audit.export", "/properties/limit", "minimum"),
    ("host.blob.put", "/properties/name", "minLength"),
    ("host.blob.put", "/properties/ttl_s", "maximum"),
    ("host.blob.put", "/properties/ttl_s", "minimum"),
    ("host.blob.touch", "/properties/ttl_s", "maximum"),
    ("host.blob.touch", "/properties/ttl_s", "minimum"),
    ("host.blob.url", "/properties/ttl_s", "maximum"),
    ("host.blob.url", "/properties/ttl_s", "minimum"),
    ("host.channel.open", "/properties/name", "minLength"),
    ("host.channel.post", "/properties/body", "minLength"),
    ("host.docs.put", "/properties/name", "minLength"),
    ("host.email.send", "/properties/subject", "minLength"),
    ("host.email.send", "/properties/to/oneOf/1", "maxItems"),
    ("host.email.send", "/properties/to/oneOf/1", "minItems"),
    ("host.email.sends", "/properties/limit", "maximum"),
    ("host.email.sends", "/properties/limit", "minimum"),
    ("host.example.propose", "/properties/title", "minLength"),
    ("host.example.propose", "/properties/tool", "minLength"),
    ("host.feedback.get", "/properties/feedback_id", "minLength"),
    ("host.feedback.send", "/properties/text", "minLength"),
    ("host.index.query", "/properties/k", "maximum"),
    ("host.index.query", "/properties/k", "minimum"),
    ("host.msg.reply", "/properties/body", "minLength"),
    ("host.msg.send", "/properties/body", "minLength"),
    ("host.msg.send", "/properties/to", "minItems"),
    ("host.note.put", "/properties/text", "minLength"),
    ("host.notify.channel_add", "/properties/target", "minLength"),
    ("host.notify.send", "/properties/text", "minLength"),
    ("host.oauth.policy_set", "/properties/access_ttl_s", "maximum"),
    ("host.oauth.policy_set", "/properties/access_ttl_s", "minimum"),
    ("host.oauth.policy_set", "/properties/refresh_ttl_s", "maximum"),
    ("host.oauth.policy_set", "/properties/refresh_ttl_s", "minimum"),
    ("host.share.caller_limit", "/properties/calls_per_day", "minimum"),
    ("host.sync.delete", "/properties/sync_id", "minLength"),
    ("host.sync.get", "/properties/sync_id", "minLength"),
    ("host.sync.run_now", "/properties/sync_id", "minLength"),
    ("host.tool.share", "/properties/rate_per_hour_per_caller", "maximum"),
    ("host.tool.share", "/properties/rate_per_hour_per_caller", "minimum"),
    ("host.tool.share", "/properties/scopes", "minItems"),
    ("host.tool_share", "/properties/rate_per_hour_per_caller", "maximum"),
    ("host.tool_share", "/properties/rate_per_hour_per_caller", "minimum"),
    ("host.tool_share", "/properties/scopes", "minItems"),
    ("host.uptime.create", "/properties/urls", "maxItems"),
    ("host.uptime.create", "/properties/urls", "minItems"),
];

fn flat(name: &str) -> String {
    name.replace('.', "_")
}

fn contract() -> Value {
    serde_json::from_slice(&std::fs::read(Path::new(ROOT).join("contracts/host-tools.v1.json")).expect("contract")).expect("json")
}

fn tool_mut<'a>(doc: &'a mut Value, name: &str) -> &'a mut Value {
    doc["tools"].as_array_mut().unwrap().iter_mut().find(|t| t["name"] == name).unwrap_or_else(|| panic!("{name} not in the contract"))
}

#[test]
fn sba_ac06_contract_carries_every_new_bound_keyword() {
    let doc = contract();
    for (tool, pointer, keyword) in ADDED {
        for name in [tool.to_string(), flat(tool)] {
            let t = doc["tools"].as_array().unwrap().iter().find(|t| t["name"] == name.as_str()).unwrap_or_else(|| panic!("{name} not in the contract"));
            let at = t["input_schema"].pointer(pointer).unwrap_or_else(|| panic!("{name} {pointer}"));
            assert!(at.get(*keyword).is_some_and(|v| v.is_number()), "{name} {pointer} carries no {keyword}: {at}");
        }
    }
}

#[test]
fn sba_ac06_contract_dump_check_and_tools_doc_have_no_diff() {
    let kinds = KindRegistry::with_builtin();
    let committed = std::fs::read(Path::new(ROOT).join("contracts/host-tools.v1.json")).expect("contract");
    assert_eq!(dump_contract_bytes(&kinds), committed, "contracts/host-tools.v1.json is stale: run `mcphost contract dump`");
    let doc = std::fs::read_to_string(Path::new(ROOT).join("docs/tools.md")).expect("docs/tools.md");
    assert_eq!(mcphost::gendocs::render_tools_markdown(&kinds), doc, "docs/tools.md is stale: run `mcphost gen-docs`");
}

#[test]
fn sba_ac06_each_narrowed_maximum_has_a_deprecation_entry() {
    let live = dump_contract(&KindRegistry::with_builtin());
    let mut before = contract();
    for (tool, pointer, keyword) in ADDED {
        for name in [tool.to_string(), flat(tool)] {
            if let Some(obj) = tool_mut(&mut before, &name)["input_schema"].pointer_mut(pointer).and_then(Value::as_object_mut) {
                obj.remove(*keyword);
            }
        }
    }
    let ledger = load_deprecations(&Path::new(ROOT).join("contracts/deprecations.json")).expect("deprecations.json");
    let unexcused = diff(&before, &live, &[], mcphost::state::now_unix());
    assert!(!unexcused.is_empty(), "the new maximums must read as narrowing without the ledger");
    for v in &unexcused {
        assert!(matches!(v, Violation::Narrowed { .. }), "{v:?}");
        assert!(
            ledger.iter().any(|d| d.narrowing_only && d.lead_time_valid() && d.path == v.path() && d.replacement.contains("PRD-mcphost-schema-bound-agreement")),
            "no narrowing_only deprecation entry names {}",
            v.path()
        );
    }
    let excused = diff(&before, &live, &ledger, mcphost::state::now_unix());
    assert!(excused.is_empty(), "with the ledger the drift check must pass: {excused:?}");
}
