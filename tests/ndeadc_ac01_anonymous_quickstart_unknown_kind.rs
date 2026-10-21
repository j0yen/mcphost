//! PRD-mcphost-onboarding-dead-ends-zero-c AC1 (P0) -- an anonymous
//! `host.quickstart {kind: "nope"}` POSTed to `/mcp` fails `unknown_kind`
//! with the same `next_call` a keyed caller gets, and creates no tenant.

use crate::common;
use common::{FixedLookup, McpClient, TestServer, signup};
use mcphost::kinds::KindRegistry;
use mcphost::kinds::http::{HttpKind, NameLookup};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;

const BODY: &str = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"host.quickstart","arguments":{"kind":"nope"}}}"#;

fn http_only() -> KindRegistry {
    let mut kinds = KindRegistry::new();
    let lookup: Arc<dyn NameLookup> = Arc::new(FixedLookup(HashMap::new()));
    kinds.register(Arc::new(HttpKind::for_test("127.0.0.1", lookup)));
    kinds
}

/// Parse a plain JSON or `data: ` SSE body into the JSON-RPC object.
fn rpc_body(text: &str) -> Value {
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        return v;
    }
    text.lines()
        .filter_map(|l| l.strip_prefix("data: "))
        .find_map(|l| serde_json::from_str::<Value>(l).ok())
        .unwrap_or_else(|| panic!("no JSON-RPC body in {text}"))
}

#[tokio::test]
async fn ndeadc_ac01() {
    let mut firsts = Vec::new();
    for kinds in [common::http_kind_registry(), http_only()] {
        let first = *kinds.names().first().expect("registry has a kind");
        firsts.push(first);
        let server = TestServer::start_with_kinds(kinds).await;
        let before = server.state.db.list_tenants().await.unwrap().len();
        let resp = reqwest::Client::new()
            .post(format!("{}/mcp", server.base_url))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .body(BODY)
            .send()
            .await
            .unwrap();
        let text = resp.text().await.unwrap();
        let v = rpc_body(&text);
        assert!(v.get("result").is_none(), "must be a JSON-RPC error: {v}");
        assert_eq!(v["error"]["data"]["error_code"], "unknown_kind", "{v}");
        let want = json!({"tool": "host.quickstart", "args": {"kind": first}});
        assert_eq!(v["error"]["data"]["next_call"], want);
        let after = server.state.db.list_tenants().await.unwrap().len();
        assert_eq!(before, after, "anonymous quickstart created a tenant");

        let (_ns, key) = signup(&server.base_url, "Ndeadc AC1 Tenant").await;
        let client = McpClient::with_bearer(&server.base_url, &key);
        let err = client
            .tools_call("host.quickstart", json!({"kind": "nope"}))
            .await
            .expect_err("keyed unknown kind fails");
        assert_eq!(err.data["next_call"], want, "keyed next_call differs");
    }
    assert_ne!(firsts[0], firsts[1], "registries must differ in names().first()");
}
