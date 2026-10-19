//! contract: reads registry handler::host_tool_descriptors
//! PRD-mcphost-input-schema-is-the-validator
//! AC2 (P0) -- Given host.notify.send, host.table.model_set, host.policy.set
//! and host.tool.share, When their schemas are read, Then `level`, `key`,
//! `target` and `visibility` carry `enum` equal to the handler's accepted
//! set, built from the same constant, with one test per pair asserting
//! equality.

use mcphost::handler::host_tool_descriptors;
use mcphost::kinds::KindRegistry;
use mcphost::notify::Level;
use mcphost::rowpolicy::policy::PolicyTarget;
use mcphost::sharing::Visibility;
use mcphost::tables_model::ANNOTATION_KEYS;
use serde_json::Value;

fn property(tool: &str, arg: &str) -> Value {
    let tools = host_tool_descriptors(&KindRegistry::with_builtin());
    let tool = tools.iter().find(|t| t.name.as_ref() == tool).unwrap_or_else(|| panic!("{tool} not listed"));
    tool.input_schema["properties"][arg].clone()
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().unwrap_or_else(|| panic!("not an array: {v}")).iter().map(|s| s.as_str().expect("string").to_string()).collect()
}

#[test]
fn notify_send_level_enum_equals_level_names() {
    let got = strings(&property("host.notify.send", "level")["enum"]);
    let want: Vec<String> = Level::names().iter().map(|s| s.to_string()).collect();
    assert_eq!(got, want);
    // Every advertised value is one the handler parses.
    assert!(got.iter().all(|l| Level::parse(l).is_some()));
}

#[test]
fn table_model_set_key_enum_equals_annotation_keys() {
    let got = strings(&property("host.table.model_set", "key")["enum"]);
    let want: Vec<String> = ANNOTATION_KEYS.iter().map(|s| s.to_string()).collect();
    assert_eq!(got, want);
}

#[test]
fn policy_set_target_kinds_equal_policy_target_kinds() {
    let target = property("host.policy.set", "target");
    assert_eq!(target["type"], "object");
    let got = strings(&target["propertyNames"]["enum"]);
    let want: Vec<String> = PolicyTarget::KINDS.iter().map(|s| s.to_string()).collect();
    assert_eq!(got, want);
    // The handler's own variants are exactly this set.
    for kind in PolicyTarget::KINDS {
        let t = PolicyTarget::from_kind_value(kind, "x".into()).unwrap_or_else(|| panic!("{kind} not a variant"));
        assert_eq!(t.kind(), kind);
    }
    assert!(PolicyTarget::from_kind_value("other", "x".into()).is_none());
}

#[test]
fn tool_share_visibility_enum_equals_shareable() {
    let got = strings(&property("host.tool.share", "visibility")["enum"]);
    let want: Vec<String> = Visibility::SHAREABLE.iter().map(|v| v.as_str().to_string()).collect();
    assert_eq!(got, want);
    assert_eq!(Visibility::schema_enum(&Visibility::SHAREABLE), property("host.tool.share", "visibility")["enum"]);
}
