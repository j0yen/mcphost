//! PRD-mcphost-onboarding-dead-ends-zero AC3 (P0) -- Given
//! `host.tool_publish {kind: "nope"}`, When it fails, Then `next_call ==
//! {tool: "host.quickstart", args: {kind: <first registry kind>}}` and the
//! kind comes from `KindRegistry`, not a literal.

use crate::common;
use common::{FixedLookup, McpClient, TestServer, signup};
use mcphost::kinds::KindRegistry;
use mcphost::kinds::http::{HttpKind, NameLookup};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;

fn http_only() -> KindRegistry {
    let mut kinds = KindRegistry::new();
    let lookup: Arc<dyn NameLookup> = Arc::new(FixedLookup(HashMap::new()));
    kinds.register(Arc::new(HttpKind::for_test("127.0.0.1", lookup)));
    kinds
}

async fn publish_nope_next_call(kinds: KindRegistry) -> Value {
    let server = TestServer::start_with_kinds(kinds).await;
    let (_ns, key) = signup(&server.base_url, "Ndead AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let err = client
        .tools_call("host.tool_publish", json!({"name": "nope_tool", "kind": "nope"}))
        .await
        .expect_err("kind 'nope' is not registered");
    assert_eq!(err.error_code.as_deref(), Some("unknown_kind"));
    err.data["next_call"].clone()
}

#[tokio::test]
async fn ndead_ac03_unknown_kind_next_call_names_the_first_registry_kind() {
    for kinds in [common::http_kind_registry(), http_only()] {
        let first = *kinds.names().first().expect("registry has a kind");
        let next = publish_nope_next_call(kinds).await;
        assert_eq!(next, json!({"tool": "host.quickstart", "args": {"kind": first}}));
    }
}

#[test]
fn ndead_ac03_kind_is_not_a_literal_in_the_table() {
    let src = include_str!("../src/next_call.rs");
    for kind in ["python", "\"echo\"", "\"http\"", "\"js\"", "\"wasm\""] {
        assert!(!src.contains(kind), "src/next_call.rs hard-codes the kind {kind}");
    }
}
