//! PRD-mcphost-same-user-second-agent
//! AC7 — Given ten concurrent first calls on one join URL with `max_uses: 5`,
//! When they complete, Then exactly 5 client rows exist and 5 calls got
//! `join_code_invalid`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sibl_ac07_concurrent_joins_land_exactly_max_uses() {
    let server = TestServer::start().await;
    let base = server.base_url.clone();
    let (ns, key) = signup(&base, "Racing Host").await;
    let first = McpClient::with_bearer(&base, &key);
    let me = extract_structured(&first.tools_call("host.whoami", json!({})).await.expect("whoami"));
    let join_path = me["join_url"].as_str().expect("join_url").trim_start_matches(&base).to_string();
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("lookup").expect("tenant");
    server.state.db.set_join_code_max_uses(tenant.id, 5).await.expect("max_uses 5");

    let calls = (0..10).map(|_| {
        let base = base.clone();
        let path = join_path.clone();
        async move { McpClient::new(&base).with_path(&path).tools_call("host.whoami", json!({})).await }
    });
    let results = futures_util::future::join_all(calls).await;

    let ok = results.iter().filter(|r| r.is_ok()).count();
    let invalid = results
        .iter()
        .filter(|r| r.as_ref().err().is_some_and(|e| e.error_code.as_deref() == Some("join_code_invalid")))
        .count();
    assert_eq!(ok, 5, "exactly max_uses calls join: {results:?}");
    assert_eq!(invalid, 5, "the other five get join_code_invalid: {results:?}");

    let rows = server.state.db.list_live_clients(tenant.id).await.expect("clients");
    assert_eq!(rows.len(), 6, "client 0 plus exactly five siblings: {rows:?}");
}
