//! PRD-mcphost-kind-ask-routing
//! AC1 (P0) -- Given `host.tool_publish {kind: docs}`, When validated, Then
//! the error is `kind 'docs' is not registered` with `did_you_mean:
//! {outcome: docs, verbs: [host.docs.put, host.docs.search], example: {..}}`.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, all_kinds_registry, signup};
use serde_json::json;

#[tokio::test]
async fn tool_publish_kind_docs_answers_with_the_docs_outcome() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(all_kinds_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Kindroute Ask AC1 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "my_tool", "kind": "docs", "spec": {}}),
        )
        .await
        .expect_err("docs is an outcome word, not a registered kind");

    assert_eq!(err.error_code.as_deref(), Some("unknown_kind"));
    assert!(
        err.message.contains("kind 'docs' is not registered"),
        "message: {}",
        err.message
    );
    // The registered-kinds list stays.
    assert!(err.data["registered"].as_array().is_some_and(|r| !r.is_empty()));

    let dym = &err.data["did_you_mean"];
    assert_eq!(dym["outcome"], json!("docs"));
    assert_eq!(dym["verbs"], json!(["host.docs.put", "host.docs.search"]));
    assert_eq!(dym["example"]["tool"], json!("host.docs.put"));
    assert!(dym["example"]["args"].is_object(), "example: {}", dym["example"]);
}
