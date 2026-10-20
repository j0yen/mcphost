//! PRD-mcphost-acting-for-in-ledger-and-audit
//! AC4 (P0) — Given 9 primary calls, 41 `tester` calls and 3 `deployer`
//! calls in the window, When `host.usage {by: "key", window: "1h"}` runs,
//! Then it returns exactly three rows with those counts and `(primary)`
//! first; `host.usage {}` is byte-identical to before the PRD.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

async fn delegate(base: &str, client: &McpClient, label: &str, ttl_s: i64) -> McpClient {
    let out: Value = extract_structured(
        &client
            .tools_call(
                "host.key.delegate",
                json!({"scopes": ["tools:call"], "ttl_s": ttl_s, "label": label, "can_delegate": true}),
            )
            .await
            .unwrap_or_else(|e| panic!("delegate {label}: {e:?}")),
    );
    McpClient::with_bearer(base, out["key"].as_str().expect("key"))
}

#[tokio::test]
async fn actfor_ac04_usage_by_key_splits_primary_and_children_and_leaves_plain_usage_alone() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "ActFor Usage Tenant").await;
    let primary = McpClient::with_bearer(&server.base_url, &key);
    primary
        .tools_call("host.tool.publish", json!({"name": "ping", "kind": "echo", "spec": {"schema": {"type": "object"}}}))
        .await
        .expect("publish");
    let tester = delegate(&server.base_url, &primary, "tester", 3000).await;
    let deployer = delegate(&server.base_url, &primary, "deployer", 2000).await;
    let tool = format!("{ns}.ping");
    for _ in 0..9 {
        primary.tools_call(&tool, json!({})).await.expect("primary call");
    }
    for _ in 0..41 {
        tester.tools_call(&tool, json!({})).await.expect("tester call");
    }
    for _ in 0..3 {
        deployer.tools_call(&tool, json!({})).await.expect("deployer call");
    }

    let usage = extract_structured(
        &primary
            .tools_call("host.usage", json!({"by": "key", "window": "1h", "tool": "ping"}))
            .await
            .expect("usage by key"),
    );
    let rows = usage["rows"].as_array().expect("rows");
    let seen: Vec<(String, i64)> = rows
        .iter()
        .map(|r| (r["label"].as_str().expect("label").to_string(), r["calls"].as_i64().expect("calls")))
        .collect();
    assert_eq!(
        seen,
        [("(primary)".to_string(), 9), ("tester".to_string(), 41), ("deployer".to_string(), 3)],
        "{usage}"
    );

    // Without `by`, the response keeps its pre-PRD key set (no key axis leaks in).
    let plain = extract_structured(&primary.tools_call("host.usage", json!({})).await.expect("plain usage"));
    let plain = plain.as_object().expect("object");
    assert!(plain.get("rows").is_none() && plain.get("by").is_none(), "{plain:?}");
    assert!(plain.get("calls").is_some() && plain.get("window").is_some(), "{plain:?}");
}
