//! PRD-mcphost-host-args-validated-by-default
//! AC6 (P0) -- Given `handler.rs`, When the descriptor scan runs, Then no
//! descriptor property carries an inline `"enum": [` literal, each former
//! literal is a `pub const` the descriptor publishes (one equality test per
//! pair), and a planted inline literal in a fixture copy fails naming the line.

use mcphost::handler::{admin_tool_descriptors, host_tool_descriptors};
use mcphost::kinds::KindRegistry;
use serde_json::{Value, json};

/// `line:text` of every non-comment line spelling an inline enum literal.
fn inline_enum_literals(source: &str) -> Vec<String> {
    source
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim_start().starts_with("//") && l.contains("\"enum\": ["))
        .map(|(i, l)| format!("handler.rs:{}: {}", i + 1, l.trim()))
        .collect()
}

#[test]
fn vbd_ac06_handler_rs_spells_no_inline_enum_literal() {
    let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/handler.rs")).expect("read handler.rs");
    let hits = inline_enum_literals(&source);
    assert!(hits.is_empty(), "inline enum literals (each must be a const the handler reads):\n{}", hits.join("\n"));
}

#[test]
fn vbd_ac06_a_planted_inline_literal_fails_naming_the_line() {
    let fixture = "fn d() {\n    json!({\"mode\": {\"type\": \"string\", \"enum\": [\"a\", \"b\"]}})\n}\n";
    let hits = inline_enum_literals(fixture);
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert!(hits[0].starts_with("handler.rs:2:"), "{hits:?}");
}

fn descriptor_enum(tool: &str, property: &str) -> Value {
    host_tool_descriptors(&KindRegistry::with_builtin())
        .into_iter()
        .chain(admin_tool_descriptors())
        .find(|t| t.name == tool)
        .unwrap_or_else(|| panic!("no descriptor {tool}"))
        .input_schema["properties"][property]["enum"]
        .clone()
}

/// One equality per former literal: the descriptor's enum IS the constant.
#[test]
fn vbd_ac06_every_former_literal_is_its_constant() {
    let pairs: Vec<(&str, &str, Value)> = vec![
        ("host.blob.get", "as", json!(mcphost::blobs::GET_AS)),
        ("host.webhook_inbox.create", "verify", json!(mcphost::webhook_inbox::VERIFY_SCHEMES)),
        ("host.job.timeline", "format", json!(mcphost::timeline::FORMATS)),
        ("host.agent.profile_set", "contact_policy", json!(mcphost::agents::CONTACT_POLICIES)),
        ("host.ask.list", "status", json!(mcphost::asks::STATUSES)),
        ("host.agent.contacts", "status", json!(mcphost::consent::STATUSES)),
        ("host.oauth.policy_set", "clients", json!(mcphost::oauth_policy::CLIENTS_MODES)),
        ("admin.tenants", "synthetic", json!(mcphost::admin::SYNTHETIC_FILTERS)),
        ("admin.ban.add", "subject_kind", json!(mcphost::admin::BAN_SUBJECT_KINDS)),
        ("admin.ban.list", "subject_kind", json!(mcphost::admin::BAN_SUBJECT_KINDS)),
        ("admin.example_proposals", "action", json!(mcphost::admin::PROPOSAL_ACTIONS)),
        ("admin.example_proposals", "status", json!(mcphost::admin::PROPOSAL_STATUSES)),
        ("admin.alerts.raise", "severity", json!(mcphost::alerts::SEVERITIES)),
        ("admin.incident.open", "impact", json!(mcphost::admin::INCIDENT_IMPACTS)),
        ("admin.status.sample", "component", json!(mcphost::statusfeed::COMPONENTS)),
    ];
    // The sixteenth, `host.tool_from_description.kind`, is listed only when a
    // drafter is configured (not in the static descriptor set), so it is pinned
    // by its source line: the descriptor publishes the constant the handler checks.
    let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/handler.rs")).expect("read handler.rs");
    assert!(source.contains("\"enum\": crate::drafter::DRAFT_KINDS"), "host.tool_from_description.kind");
    assert_eq!(pairs.len() + 1, 16, "the 16 inline literals of 137048a3");
    for (tool, property, constant) in pairs {
        assert_eq!(descriptor_enum(tool, property), constant, "{tool}.{property}");
    }
}
