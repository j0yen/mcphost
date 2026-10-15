//! PRD-mcphost-host-changelog-since-b AC3 — Given `since: "banana"`, When
//! `host.changelog` runs, Then `args_invalid` names the field and the
//! semver shape.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn chgb_ac03_banana_since_is_args_invalid_naming_field_and_semver() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "CHGB AC3").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call("host.changelog", json!({"since": "banana"}))
        .await
        .expect_err("a non-semver since must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{err:?}");
    assert_eq!(err.data["argument"], "since", "{err:?}");
    assert_eq!(err.data["field"], "since", "{err:?}");
    let text = format!("{err:?}");
    assert!(text.contains("semver") && text.contains("<major>.<minor>.<patch>"), "{text}");
}
