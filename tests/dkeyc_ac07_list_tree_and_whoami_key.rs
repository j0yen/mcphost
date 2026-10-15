//! PRD-mcphost-key-delegate-child-keys
//! AC7 (P1) — Given the tree above, When `host.key.list` runs, Then it
//! returns `c1` with `children: [c2 → children: [c3]]`, and `host.whoami`
//! under `c2` returns `key.kind == "delegated"`, `key.parent == "c1"`,
//! `key.depth == 2`.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

async fn delegate(base: &str, client: &McpClient, label: &str, ttl_s: i64) -> McpClient {
    let out = extract_structured(
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
async fn dkeyc_ac07_list_renders_the_tree_and_whoami_names_parent_and_depth() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Delegate Tree Tenant").await;
    let primary = McpClient::with_bearer(&server.base_url, &key);
    let c1 = delegate(&server.base_url, &primary, "c1", 3000).await;
    let c2 = delegate(&server.base_url, &c1, "c2", 2000).await;
    let _c3 = delegate(&server.base_url, &c2, "c3", 1000).await;

    let listed = extract_structured(&primary.tools_call("host.key.list", json!({})).await.expect("list"));
    let roots = listed["keys"].as_array().expect("keys");
    assert_eq!(roots.len(), 1, "{listed}");
    let n1 = &roots[0];
    assert_eq!(n1["label"], json!("c1"), "{listed}");
    assert_eq!(n1["depth"], json!(1), "{listed}");
    assert_eq!(n1["parent"], json!(null), "{listed}");
    assert_eq!(n1["scopes"], json!(["tools:call"]), "{listed}");
    let n2 = &n1["children"][0];
    assert_eq!(n2["label"], json!("c2"), "{listed}");
    assert_eq!(n2["parent"], json!("c1"), "{listed}");
    assert_eq!(n1["children"].as_array().unwrap().len(), 1, "{listed}");
    let n3 = &n2["children"][0];
    assert_eq!(n3["label"], json!("c3"), "{listed}");
    assert_eq!(n3["children"], json!([]), "{listed}");

    let me = extract_structured(&c2.tools_call("host.whoami", json!({})).await.expect("whoami"));
    assert_eq!(me["key"]["kind"], json!("delegated"), "{me}");
    assert_eq!(me["key"]["parent"], json!("c1"), "{me}");
    assert_eq!(me["key"]["depth"], json!(2), "{me}");
    assert_eq!(me["key"]["label"], json!("c2"), "{me}");

    // The primary's own whoami carries no `key` object.
    let primary_me = extract_structured(&primary.tools_call("host.whoami", json!({})).await.expect("whoami"));
    assert!(primary_me.get("key").is_none(), "{primary_me}");
}
