//! PRD-mcphost-onboarding-dead-ends-zero-c AC2 (P0) -- anonymous
//! `host.quickstart` with a valid kind, an alias, an outcome word or no kind
//! keeps today's response and creates no tenant.

use crate::common;
use common::{FixedLookup, TestServer, TempDataDir};
use mcphost::kinds::http::{HttpKind, NameLookup};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;

async fn call(base: &str, args: Value) -> Value {
    let body = json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
        "params":{"name":"host.quickstart","arguments":args}});
    let text = reqwest::Client::new()
        .post(format!("{base}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(body.to_string())
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    if let Ok(v) = serde_json::from_str::<Value>(&text) {
        return v;
    }
    text.lines()
        .filter_map(|l| l.strip_prefix("data: "))
        .find_map(|l| serde_json::from_str::<Value>(l).ok())
        .unwrap_or_else(|| panic!("no JSON-RPC body in {text}"))
}

#[tokio::test]
async fn ndeadc_ac02() {
    let dir = TempDataDir::new();
    let mut kinds = common::python_kind_registry(&dir.0);
    let lookup: Arc<dyn NameLookup> = Arc::new(FixedLookup(HashMap::new()));
    kinds.register(Arc::new(HttpKind::for_test("127.0.0.1", lookup)));
    let server = TestServer::start_with_kinds(kinds).await;
    let before = server.state.db.list_tenants().await.unwrap().len();

    for kind in ["python", "webhook", "docs"] {
        let v = call(&server.base_url, json!({"kind": kind})).await;
        let r = &v["result"];
        assert!(r.is_object(), "{kind}: no result: {v}");
        assert_ne!(r["isError"], json!(true), "{kind}: {v}");
        let sc = &r["structuredContent"];
        assert_eq!(sc["authenticated"], json!(false), "{kind}: {v}");
        assert_eq!(sc["steps"][0]["call"], "signup", "{kind}: {v}");
    }
    let v = call(&server.base_url, json!({})).await;
    let kinds = v["result"]["structuredContent"]["kinds"].as_array().expect("kinds index");
    assert!(kinds.iter().any(|k| k == "python"), "{v}");

    let after = server.state.db.list_tenants().await.unwrap().len();
    assert_eq!(before, after, "anonymous quickstart created a tenant");
}
