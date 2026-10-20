//! PRD-mcphost-org-cost-attribution
//! AC7 — Given a member removed mid-window, When the owner reads `members`,
//! Then the row is present with `removed: true` and its calls up to removal.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn orgc_ac07() {
    let server = TestServer::start().await;
    let (_owner_ns, owner_key) = signup(&server.base_url, "Owner").await;
    let owner = McpClient::with_bearer(&server.base_url, &owner_key);
    let created = extract_structured(&owner.tools_call("host.org.create", json!({"name": "acme"})).await.expect("create"));
    let code = created["invite_url"].as_str().expect("url").trim_end_matches("/mcp").rsplit('/').next().expect("code").to_string();
    let (ann_ns, ann_key) = signup(&server.base_url, "Ann").await;
    let ann = McpClient::with_bearer(&server.base_url, &ann_key);
    ann.tools_call("host.org.join", json!({"code": code})).await.expect("join");
    let ann_id = server.state.db.find_tenant_by_namespace(ann_ns.clone()).await.expect("lookup").expect("ann").id;

    let record = |n: usize| {
        let db = server.state.db.clone();
        async move {
            for _ in 0..n {
                db.record_call(ann_id, "t".to_string(), 5, true, None, None, None, "ok", "external".to_string(), None)
                    .await
                    .expect("record_call");
            }
        }
    };
    record(3).await;
    // Move those calls to 200 s ago, remove Ann, put the removal 100 s ago,
    // then let Ann call twice more after the removal.
    let conn = rusqlite::Connection::open(server.state.db.data_dir().join("mcphost.db")).expect("open db");
    let now = mcphost::state::now_unix();
    conn.execute("UPDATE calls SET started_unix = ?2 WHERE tenant_id = ?1", rusqlite::params![ann_id, now - 200]).expect("backdate calls");
    owner.tools_call("host.org.remove", json!({"member": ann_ns})).await.expect("remove");
    conn.execute("UPDATE org_members SET removed_unix = ?2 WHERE tenant_id = ?1", rusqlite::params![ann_id, now - 100]).expect("backdate removal");
    record(2).await;

    let got = extract_structured(&owner.tools_call("host.org.usage", json!({"window": "24h"})).await.expect("host.org.usage"));
    let rows = got["members"].as_array().expect("members");
    let row = rows.iter().find(|r| r["namespace"] == json!(ann_ns)).unwrap_or_else(|| panic!("removed member row missing: {got}"));
    assert_eq!(row["removed"], json!(true), "{got}");
    assert_eq!(row["calls"], json!(3), "calls up to removal only: {got}");
    let owner_row = rows.iter().find(|r| r["removed"] == json!(false)).expect("live owner row");
    assert_eq!(owner_row["role"], json!("owner"), "{got}");
    assert_eq!(got["totals"]["calls"], json!(3), "{got}");
}
