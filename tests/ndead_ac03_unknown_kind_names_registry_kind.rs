//! PRD-mcphost-onboarding-dead-ends-zero
//! AC3 — Given `host.tool_publish {kind: "nope"}`, When it fails, Then
//! `next_call == {tool: "host.quickstart", args: {kind: <first registry
//! kind>}}` and the kind comes from `KindRegistry`, not a literal.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, all_kinds_registry, signup};
use serde_json::json;

#[tokio::test]
async fn ndead_ac03() {
    let dir = TempDataDir::new();
    let kinds = all_kinds_registry(&dir.0);
    let first = kinds.names()[0];
    assert!(kinds.names().len() > 1, "a multi-kind registry, so `first` is a choice");

    let server = TestServer::start_with_kinds(kinds).await;
    let (_ns, key) = signup(&server.base_url, "Nope").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call("host.tool_publish", json!({"name": "x_tool", "kind": "nope", "spec": {}}))
        .await
        .expect_err("an unknown kind is refused");
    assert_eq!(err.error_code.as_deref(), Some("unknown_kind"), "{}", err.data);
    assert_eq!(
        err.data["next_call"],
        json!({"tool": "host.quickstart", "args": {"kind": first}}),
        "{}",
        err.data
    );
    assert_eq!(
        server.state.kinds.names()[0],
        first,
        "the kind is the registry's own first, not a literal"
    );
}
