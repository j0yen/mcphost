//! PRD-mcphost-kind-docs-from-registry-b-v2 AC3: `host.quickstart kind=chain`
//! for a keyed tenant gains `doc_anchor` (the `DocAnchor` `host.help.get`
//! resolves) and `spec_fields` (the `KindDoc`'s names, each once); the keys
//! it carried before are unchanged.

use crate::common::{self, McpClient, TestServer, extract_structured};
use mcphost::doc_anchors::DocAnchor;
use mcphost::plans::PlanCatalog;
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// Every key a keyed `kind=chain` response carried before this PRD,
/// conditional ones (alternate hosts, delegation, aliases) included.
const KEYS_BEFORE: &[&str] = &[
    "aliases",
    "alt_endpoint",
    "authenticated",
    "bridge_returns",
    "capabilities",
    "client",
    "connections",
    "contract",
    "docs",
    "endpoint",
    "events",
    "fixtures",
    "host_steps_allowed",
    "hint",
    "install_doc_url",
    "install_links",
    "kind",
    "kinds",
    "limits",
    "namespace",
    "next",
    "next_steps",
    "oauth_entry_url",
    "org",
    "reachability",
    "recipe",
    "recipes",
    "recipes_nearby",
    "resolved_from",
    "sandbox_api",
    "starter_tool",
    "steps",
    "triggers",
    "try_before_call",
];

#[tokio::test]
async fn kdrb_ac03() {
    let server = TestServer::start_with_kinds(common::chain_kind_registry()).await;
    let (_ns, key) = common::signup(&server.base_url, "kdrb ac03").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let result = client.tools_call("host.quickstart", json!({"kind": "chain"})).await.expect("quickstart");
    let body: Value = extract_structured(&result);

    // doc_anchor: the typed anchor for the chain page's `## Spec fields`.
    let anchor = body["doc_anchor"].as_str().unwrap_or_else(|| panic!("no doc_anchor in {body}"));
    assert_eq!(anchor, "docs/kinds/chain.md#spec-fields");
    assert_eq!(DocAnchor::parse(anchor), DocAnchor::kind_spec_fields("chain"));
    let help = client.tools_call("host.help.get", json!({"anchor": anchor})).await.expect("help.get resolves it");
    let help: Value = extract_structured(&help);
    assert_eq!(help["anchor"], json!(anchor));
    assert_eq!(help["title"], json!("Spec fields"));
    // Every registered kind's page answers the same lookup.
    for kind in mcphost::kinds::production_registry().names() {
        assert!(DocAnchor::kind_spec_fields(kind).is_some(), "kind {kind} has no `## Spec fields` anchor");
    }

    // spec_fields: the KindDoc's names, each once.
    let registry = mcphost::kinds::production_registry();
    let doc = registry.get("chain").expect("chain kind").doc(&PlanCatalog::default_catalog());
    let want: Vec<String> = doc.spec_fields.iter().map(|f| f.name.clone()).collect();
    let got: Vec<String> = body["spec_fields"]
        .as_array()
        .unwrap_or_else(|| panic!("no spec_fields in {body}"))
        .iter()
        .map(|v| v.as_str().expect("a name").to_string())
        .collect();
    assert_eq!(got, want);
    assert_eq!(got.iter().collect::<BTreeSet<_>>().len(), got.len(), "each name once: {got:?}");

    // The example spec's top-level keys are all named.
    let spec = body.pointer("/steps/0/arguments/spec").and_then(Value::as_object).expect("steps[0].arguments.spec");
    for key in spec.keys() {
        assert!(got.contains(key), "example spec key `{key}` is not in spec_fields {got:?}");
    }

    // Nothing that was there moved: no key beyond the old set and the two
    // new ones, the unconditional old ones still present and unchanged.
    let keys: BTreeSet<&str> = body.as_object().unwrap().keys().map(String::as_str).collect();
    let allowed: BTreeSet<&str> = KEYS_BEFORE.iter().copied().chain(["doc_anchor", "spec_fields"]).collect();
    assert_eq!(keys.difference(&allowed).collect::<Vec<_>>(), Vec::<&&str>::new(), "unexpected new keys");
    for k in ["authenticated", "namespace", "kind", "contract", "kinds", "starter_tool", "steps", "limits", "host_steps_allowed"] {
        assert!(keys.contains(k), "`{k}` is gone from {keys:?}");
    }
    assert_eq!(body["kind"], json!("chain"));
    assert_eq!(body["host_steps_allowed"], json!(mcphost::kinds::HOST_STEPS_ALLOWED));
    assert_eq!(body["contract"], registry.get("chain").unwrap().contract().response_block());
}
