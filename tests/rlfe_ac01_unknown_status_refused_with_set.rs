//! PRD-mcphost-runs-list-filter-enums-generated
//! AC1 (P0) -- Given a `TestServer` over the production `build_router`, When
//! `host.runs.list` is called with `status: "not-a-status"`, Then the reply is
//! an `args_invalid` error whose `data` is an object with `path` `/status` and
//! `expected` equal to `RunStatus::names()`, and `status: 5` is refused with
//! the same code and an object `data`.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::runs::RunStatus;
use serde_json::json;

#[tokio::test]
async fn rlfe_ac01() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "RLFE AC1 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call("host.runs.list", json!({"status": "not-a-status"}))
        .await
        .expect_err("an unknown status must be refused, not answered with an empty page");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{err:?}");
    assert!(err.data.is_object(), "data must be an object: {:?}", err.data);
    assert_eq!(err.data["path"], json!("/status"), "{:?}", err.data);
    assert_eq!(err.data["constraint"], json!("enum"), "{:?}", err.data);
    assert_eq!(err.data["expected"], json!(RunStatus::names()), "{:?}", err.data);

    let err5 = client
        .tools_call("host.runs.list", json!({"status": 5}))
        .await
        .expect_err("a non-string status must be refused");
    assert_eq!(err5.error_code.as_deref(), Some("args_invalid"), "same code for the typed refusal: {err5:?}");
    assert!(err5.data.is_object(), "data must be an object: {:?}", err5.data);
    assert_eq!(err5.data["path"], json!("/status"), "{:?}", err5.data);
}
