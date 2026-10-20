//! PRD-mcphost-empty-call-refusal-stated-in-schema
//! contract: reads registry handler::host_tool_descriptors
//! AC4 (P0) -- Given `import::import` and `subkeys::delegate` called directly
//! with `{}`, When the unit test runs, Then each still returns its own refusal
//! (import.rs:153, subkeys.rs:907) -- defence in depth the router never
//! reaches.

use crate::common;
use mcphost::errors::AppError;
use serde_json::json;

#[tokio::test]
async fn ecs_ac04_import_and_delegate_still_refuse_an_empty_call_themselves() {
    let (state, _dir) = common::bare_app_state().await;
    let tenant = state
        .db
        .create_tenant("ECS AC4".to_string(), "t-ecs-ac4".to_string(), "key-hash-ecs-ac4".to_string(), None)
        .await
        .expect("create_tenant");

    let err = mcphost::import::import(&state, &tenant, &json!({})).await.expect_err("import {} must refuse");
    match err {
        AppError::InvalidArgs(m) => assert_eq!(m, "host.import needs 'run_id' or 'url'"),
        other => panic!("import: unexpected refusal {other:?}"),
    }

    let err = mcphost::subkeys::delegate(&state, &tenant, &json!({}), None).await.expect_err("delegate {} must refuse");
    match err {
        AppError::InvalidParams(m) => assert_eq!(m, "scopes: an array of 1.. scope names is required"),
        other => panic!("delegate: unexpected refusal {other:?}"),
    }
}
