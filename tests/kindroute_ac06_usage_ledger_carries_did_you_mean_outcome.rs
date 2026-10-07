//! PRD-mcphost-kind-ask-routing
//! AC6 (P1) -- Given a `did_you_mean` response, When the usage ledger is
//! read, Then the row carries `did_you_mean_outcome: docs`.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, all_kinds_registry, signup};
use serde_json::json;

#[tokio::test]
async fn did_you_mean_hit_lands_in_host_tool_usage_with_its_outcome() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(all_kinds_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Kindroute Ask AC6 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.unwrap().expect("tenant");

    assert!(
        server.state.db.did_you_mean_outcomes(tenant.id).await.unwrap().is_empty(),
        "no did_you_mean row before any hit"
    );

    let err = client
        .tools_call("host.tool_publish", json!({"name": "my_tool", "kind": "docs", "spec": {}}))
        .await
        .expect_err("docs is an outcome word");
    assert_eq!(err.data["did_you_mean"]["outcome"], json!("docs"));

    assert_eq!(
        server.state.db.did_you_mean_outcomes(tenant.id).await.unwrap(),
        vec!["docs".to_string()]
    );

    // A plain unknown kind (no outcome) records nothing, and the hit row is
    // not a real success: it does not count as a used tool.
    let _ = client
        .tools_call("host.tool_publish", json!({"name": "my_tool", "kind": "lambda", "spec": {}}))
        .await
        .expect_err("lambda is not an outcome");
    assert_eq!(server.state.db.did_you_mean_outcomes(tenant.id).await.unwrap().len(), 1);
    assert!(
        !server
            .state
            .db
            .host_tool_already_used(tenant.id, "did_you_mean:host.tool_publish:docs".to_string())
            .await
            .unwrap()
    );
}
