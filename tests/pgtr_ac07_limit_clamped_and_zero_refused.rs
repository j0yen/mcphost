//! PRD-mcphost-paged-trait-on-every-list-verb
//! AC7 (P1) -- Given `limit: 5000` on `host.runs.list`, When called, Then 200
//! rows return with `_meta.mcphost.limit_clamped {asked: 5000, max: 200}`;
//! Given `limit: 0`, Then `args_invalid`.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::{Value, json};

#[tokio::test]
async fn runs_list_clamps_limit_5000_to_200_with_meta_and_refuses_zero() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Pgtr AC7").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("db lookup")
        .expect("tenant exists");

    for i in 0..230 {
        server
            .state
            .db
            .insert_queued_run(
                format!("pgtr-ac7-run-{i:03}"),
                tenant.id,
                "ghost".to_string(),
                "event".to_string(),
                None,
                None,
                300,
                "{}".to_string(),
                false,
                false,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("insert_queued_run");
    }

    let result = client
        .tools_call("host.runs.list", json!({"limit": 5000}))
        .await
        .expect("a limit above the max is clamped, not refused");
    let runs = common::extract_structured(&result)["runs"].as_array().expect("runs array").len();
    assert_eq!(runs, 200, "{result:?}");
    assert_eq!(
        result["_meta"]["mcphost"]["limit_clamped"],
        json!({"asked": 5000, "max": 200}),
        "{result:?}"
    );
    assert!(
        common::extract_structured(&result).get(mcphost::paged::LIMIT_CLAMPED_KEY).is_none(),
        "the clamp carrier key must not leak into the body: {result:?}"
    );

    // Within bounds: no clamp report at all.
    let ok = client.tools_call("host.runs.list", json!({"limit": 10})).await.expect("limit 10");
    assert_eq!(common::extract_structured(&ok)["runs"].as_array().map(Vec::len), Some(10));
    assert!(ok["_meta"].get("mcphost").is_none_or(Value::is_null), "{ok:?}");

    let err = client
        .tools_call("host.runs.list", json!({"limit": 0}))
        .await
        .expect_err("limit 0 must be args_invalid");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{err:?}");
}
