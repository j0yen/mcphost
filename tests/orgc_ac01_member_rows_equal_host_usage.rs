//! PRD-mcphost-org-cost-attribution
//! AC1 — Given a fixture org of 3 members with 5, 12 and 0 calls in the
//! window, When the owner calls `host.org.usage {window: "24h"}`, Then each
//! `members[m]` equals that member's `host.usage {window: "24h"}` for
//! `calls`, `errors`, `seconds`, `state_bytes`, and `totals.calls == 17`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

async fn record(server: &TestServer, ns: &str, n: usize) {
    let tenant = server.state.db.find_tenant_by_namespace(ns.to_string()).await.expect("lookup").expect("tenant");
    for _ in 0..n {
        server
            .state
            .db
            .record_call(tenant.id, "t".to_string(), 5, true, None, None, None, "ok", "external".to_string(), None)
            .await
            .expect("record_call");
    }
}

#[tokio::test]
async fn orgc_ac01() {
    let server = TestServer::start().await;
    let (owner_ns, owner_key) = signup(&server.base_url, "Owner").await;
    let owner = McpClient::with_bearer(&server.base_url, &owner_key);
    let created = extract_structured(&owner.tools_call("host.org.create", json!({"name": "acme"})).await.expect("create"));
    let code = created["invite_url"].as_str().expect("url").trim_end_matches("/mcp").rsplit('/').next().expect("code").to_string();

    let mut members = vec![(owner_ns.clone(), owner)];
    for who in ["Ann", "Bob"] {
        let (ns, key) = signup(&server.base_url, who).await;
        let c = McpClient::with_bearer(&server.base_url, &key);
        c.tools_call("host.org.join", json!({"code": code})).await.expect("join");
        members.push((ns, c));
    }
    // 5, 12 and 0 calls.
    record(&server, &members[0].0, 5).await;
    record(&server, &members[1].0, 12).await;

    let got = extract_structured(
        &members[0].1.tools_call("host.org.usage", json!({"window": "24h"})).await.expect("host.org.usage"),
    );
    let rows = got["members"].as_array().expect("members");
    assert_eq!(rows.len(), 3, "{got}");
    for (ns, client) in &members {
        let own = extract_structured(&client.tools_call("host.usage", json!({"window": "24h"})).await.expect("host.usage"));
        let row = rows.iter().find(|r| r["namespace"] == json!(ns)).unwrap_or_else(|| panic!("no row for {ns}: {got}"));
        for key in ["calls", "errors", "state_bytes"] {
            assert_eq!(row[key], own[key], "{ns}.{key}: {row} vs {own}");
        }
        let seconds = own["jobs"]["seconds"].as_i64().unwrap_or(0) + own["scheduled"]["seconds"].as_i64().unwrap_or(0);
        assert_eq!(row["seconds"], Value::from(seconds), "{ns}.seconds");
    }
    assert_eq!(got["totals"]["calls"], json!(17), "{got}");
}
