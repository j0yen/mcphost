//! PRD-mcphost-org-cost-attribution
//! AC3 — Given 2 org tools with 3 and 1 callers, When the owner reads
//! `tools`, Then rows carry `name`, `publisher`, `calls`, `callers` and sort
//! by `calls` descending.

use crate::common;
use common::{McpClient, TestServer, extract_structured, publish, signup};
use serde_json::json;

#[tokio::test]
async fn orgc_ac03() {
    let server = TestServer::start().await;
    let (owner_ns, owner_key) = signup(&server.base_url, "Owner").await;
    let owner = McpClient::with_bearer(&server.base_url, &owner_key);
    let created = extract_structured(&owner.tools_call("host.org.create", json!({"name": "acme"})).await.expect("create"));
    let code = created["invite_url"].as_str().expect("url").trim_end_matches("/mcp").rsplit('/').next().expect("code").to_string();
    let spec = json!({"schema": {"type": "object", "properties": {}, "required": []}});
    let mut ids = Vec::new();
    let mut nss = Vec::new();
    for who in ["Ann", "Bob"] {
        let (ns, key) = signup(&server.base_url, who).await;
        let c = McpClient::with_bearer(&server.base_url, &key);
        c.tools_call("host.org.join", json!({"code": code})).await.expect("join");
        if who == "Ann" {
            publish(&c, "beta", "echo", spec.clone()).await;
        }
        ids.push(server.state.db.find_tenant_by_namespace(ns.clone()).await.expect("lookup").expect("tenant").id);
        nss.push(ns);
    }
    publish(&owner, "alpha", "echo", spec).await;
    let owner_id = server.state.db.find_tenant_by_namespace(owner_ns.clone()).await.expect("lookup").expect("owner").id;

    let call = |publisher: i64, tool: &'static str, caller: Option<i64>| {
        let db = server.state.db.clone();
        async move {
            db.record_call_attributed(publisher, tool.to_string(), 5, true, None, None, None, "ok", "external".to_string(), None, caller)
                .await
                .expect("record_call_attributed");
        }
    };
    // alpha: three distinct callers (itself, Ann, Bob); beta: one caller.
    call(owner_id, "alpha", None).await;
    call(owner_id, "alpha", Some(ids[0])).await;
    call(owner_id, "alpha", Some(ids[1])).await;
    call(ids[0], "beta", None).await;

    let got = extract_structured(&owner.tools_call("host.org.usage", json!({"window": "24h"})).await.expect("host.org.usage"));
    let tools = got["tools"].as_array().expect("tools");
    assert_eq!(tools.len(), 2, "{got}");
    assert_eq!(tools[0]["name"], json!(format!("{owner_ns}.alpha")), "{got}");
    assert_eq!(tools[0]["publisher"], json!(owner_ns), "{got}");
    assert_eq!((tools[0]["calls"].as_i64(), tools[0]["callers"].as_i64()), (Some(3), Some(3)), "{got}");
    assert_eq!(tools[1]["name"], json!(format!("{}.beta", nss[0])), "{got}");
    assert_eq!((tools[1]["calls"].as_i64(), tools[1]["callers"].as_i64()), (Some(1), Some(1)), "{got}");
}
