//! PRD-mcphost-tool-call-host-verb-forward
//! AC6 — Given every hint string the server can emit (quickstart kinds,
//! next-hint table, `trigger_set_hint`), When the hint-parity test runs,
//! Then every verb named in a hint resolves through the normalizer and
//! each hint prints the underscore tool name once.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, all_kinds_registry, extract_structured, signup};
use mcphost::kinds::KindRegistry;
use serde_json::json;

fn count_occurrences(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

/// `trigger_set_hint` (`control::quickstart`'s per-alias `hint` field):
/// every alias word (`kindroute`'s own alias table) resolves to the http
/// recipe and carries this hint, which names `host.trigger.set` and must
/// now print `host_trigger_set` exactly once alongside it.
#[tokio::test]
async fn every_quickstart_alias_hint_names_both_forms_of_trigger_set() {
    let kinds = KindRegistry::with_builtin();
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(all_kinds_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "AC6 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    for alias in mcphost::kinds::aliases::alias_names() {
        let structured = extract_structured(
            &client
                .tools_call("host.quickstart", json!({"kind": alias}))
                .await
                .unwrap_or_else(|e| panic!("host.quickstart(kind=\"{alias}\") must resolve: {} {}", e.code, e.message)),
        );
        let hint = structured["hint"]
            .as_str()
            .unwrap_or_else(|| panic!("quickstart(kind=\"{alias}\") must carry a hint: {structured}"));
        assert!(
            hint.contains("host.trigger.set"),
            "alias {alias}'s hint must name host.trigger.set: {hint}"
        );
        assert_eq!(
            mcphost::verbforward::resolve("host.trigger.set", &kinds).as_deref(),
            Some("host.trigger.set"),
            "host.trigger.set named in alias {alias}'s hint must resolve through the normalizer"
        );
        assert_eq!(
            count_occurrences(hint, "host_trigger_set"),
            1,
            "alias {alias}'s hint must print the underscore tool name exactly once: {hint}"
        );
    }
}

/// `host.quickstart()`'s own `triggers` sentence (present regardless of
/// `kind`) names `host.trigger.set` too -- same parity requirement.
#[tokio::test]
async fn quickstart_triggers_sentence_names_both_forms() {
    let kinds = KindRegistry::with_builtin();
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(all_kinds_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "AC6 Triggers Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let structured = extract_structured(
        &client
            .tools_call("host.quickstart", json!({}))
            .await
            .expect("host.quickstart()"),
    );
    let triggers = structured["triggers"]
        .as_str()
        .unwrap_or_else(|| panic!("host.quickstart() must carry a triggers sentence: {structured}"));
    assert!(triggers.contains("host.trigger.set"), "{triggers}");
    assert_eq!(
        mcphost::verbforward::resolve("host.trigger.set", &kinds).as_deref(),
        Some("host.trigger.set")
    );
    assert_eq!(
        count_occurrences(triggers, "host_trigger_set"),
        1,
        "the triggers sentence must print the underscore tool name exactly once: {triggers}"
    );
}

/// `control::next_hint_for`'s own static table: every `from`/`tool` it can
/// ever name is a real host verb the normalizer resolves -- this is the
/// structured `next: {tool, why}` field, not printed prose, so only the
/// first half of the parity requirement (resolves through the normalizer)
/// applies; there is no underscore form to print in a machine field.
#[test]
fn every_next_hint_table_entry_resolves_through_the_normalizer() {
    let kinds = KindRegistry::with_builtin();
    for (from, tool, _why) in mcphost::control::next_hint_table() {
        assert!(
            mcphost::verbforward::resolve(from, &kinds).is_some(),
            "next-hint table's own key {from} must resolve through the normalizer"
        );
        assert!(
            mcphost::verbforward::resolve(tool, &kinds).is_some(),
            "next-hint table's suggested tool {tool} (from {from}) must resolve through the normalizer"
        );
    }
}
