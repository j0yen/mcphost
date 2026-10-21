//! PRD-mcphost-acting-for-live-proof
//! AC1 (P0) — Given a published echo tool and a depth-1 child key
//! (`tools:call`, label `kid`), When the child calls `host.whoami` three
//! times and the published tool once by wire name, Then
//! `host.usage {by: "key", window: "1h"}` has a `kid` row with `calls == 1`
//! and every whoami response carries `_meta.acting_for.label == "kid"`.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

#[tokio::test]
async fn actforlive_characterization_control_verbs_uncounted() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "ActForLive Tenant").await;
    let primary = McpClient::with_bearer(&server.base_url, &key);
    primary
        .tools_call("host.tool.publish", json!({"name": "actfor_probe", "kind": "echo", "spec": {"schema": {"type": "object"}}}))
        .await
        .expect("publish");
    let delegated: Value = extract_structured(
        &primary
            .tools_call("host.key.delegate", json!({"scopes": ["tools:call"], "ttl_s": 600, "label": "kid"}))
            .await
            .expect("delegate"),
    );
    let kid = McpClient::with_bearer(&server.base_url, delegated["key"].as_str().expect("key"));

    for _ in 0..3 {
        let res = kid.tools_call("host.whoami", json!({})).await.expect("whoami");
        assert_eq!(res["_meta"]["acting_for"]["label"], "kid", "{res}");
    }
    kid.tools_call(&format!("{ns}.actfor_probe"), json!({})).await.expect("probe call");

    let usage = extract_structured(
        &primary.tools_call("host.usage", json!({"by": "key", "window": "1h"})).await.expect("usage by key"),
    );
    let rows = usage["rows"].as_array().expect("rows");
    let kid_rows: Vec<&Value> = rows.iter().filter(|r| r["label"] == "kid").collect();
    assert_eq!(kid_rows.len(), 1, "{usage}");
    assert_eq!(kid_rows[0]["calls"].as_i64(), Some(1), "control verbs must not count: {usage}");
}
