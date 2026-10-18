//! PRD-mcphost-count-and-total-semantics
//! AC3 -- Given `host.runs.list(verdict="exceeded")` over 60 runs with 7
//! failures, When page 1 returns, Then `total: 7`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup, vord_seed_runs};
use serde_json::json;

#[tokio::test]
async fn ctot_ac03_runs_list_verdict_total() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Ctot AC3 Tenant").await;
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("find").expect("tenant");
    vord_seed_runs(&server, tenant.id, 60, 7).await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let page = client
        .tools_call("host.runs.list", json!({"verdict": "exceeded", "limit": 5}))
        .await
        .expect("runs.list ok");
    let page = extract_structured(&page);
    assert_eq!(page["runs"].as_array().expect("runs").len(), 5, "{page:?}");
    assert_eq!(page["total"], json!(7), "{page:?}");
    assert!(page.get("total_estimate").is_none(), "{page:?}");

    // Unfiltered: all 60 runs, still exact.
    let all = client.tools_call("host.runs.list", json!({})).await.expect("runs.list ok");
    assert_eq!(extract_structured(&all)["total"], json!(60));
}
