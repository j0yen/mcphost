//! PRD-mcphost-chain-host-steps AC3 (P0) — Given a chain spec whose step
//! names `host.tool_remove`, When published, Then the error class is
//! `step_tool_not_allowed` and `data.allowed` equals the allowlist returned
//! by `host.quickstart kind=chain`.

use crate::common;
use common::{TestServer, chain_kind_registry, signup};
use serde_json::json;

#[tokio::test]
async fn host_tool_remove_step_is_refused_matching_the_quickstart_allowlist() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Chain Disallowed Step Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let quickstart = common::extract_structured(
        &client
            .tools_call("host.quickstart", json!({"kind": "chain"}))
            .await
            .expect("host.quickstart must succeed"),
    );
    let allowed = quickstart["host_steps_allowed"]
        .as_array()
        .expect("host_steps_allowed array")
        .clone();
    assert!(
        !allowed.iter().any(|v| v == "host.tool_remove"),
        "host.tool_remove must not be on the allowlist: {allowed:?}"
    );

    let err = client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "pipeline",
                "kind": "chain",
                "spec": {"steps": [{"tool": "host.tool_remove", "args": {}}]},
            }),
        )
        .await
        .expect_err("a chain naming host.tool_remove as a step must be refused");

    assert_eq!(err.error_code.as_deref(), Some("step_tool_not_allowed"));
    assert_eq!(
        err.data["allowed"], json!(allowed),
        "the publish rejection's data.allowed must equal host.quickstart's own host_steps_allowed: {err:?}"
    );
}
