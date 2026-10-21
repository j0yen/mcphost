//! PRD-mcphost-onboarding-dead-ends-zero-b AC3 (P0) -- Given an anonymous
//! session that is rate-limited on implicit signup, When the error returns,
//! Then `next_call.tool == "host.quickstart"` with `args.retry_after_s` equal
//! to the limiter's value.

use crate::common;
use common::{McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn ndeadb_ac03_signup_rate_limited_next_call() {
    let server = TestServer::start_with_signup_rate_limit(1).await;
    let publish = json!({
        "name": "hello",
        "kind": "echo",
        "spec": {"schema": {"type": "object", "properties": {"msg": {"type": "string"}}, "required": ["msg"]}},
    });
    McpClient::new(&server.base_url)
        .with_session_continuity()
        .tools_call("host.tool_publish", publish.clone())
        .await
        .expect("the first implicit signup takes the one slot");

    // A distinct User-Agent: the limiter is reached, not the same-peer block
    // (PRD-mcphost-implicit-signup-mints-only-on-success R3).
    let err = McpClient::new(&server.base_url)
        .with_session_continuity()
        .with_user_agent("ndeadb-ac03-second")
        .tools_call("host.tool_publish", publish)
        .await
        .expect_err("the second anonymous session is over the cap");
    assert_eq!(err.error_code.as_deref(), Some("signup_rate_limited"), "{err:?}");
    let next = &err.data["next_call"];
    assert_eq!(next["tool"], "host.quickstart", "{err:?}");
    assert_eq!(next["args"]["retry_after_s"], err.data["retry_after_s"], "{err:?}");
    assert_eq!(next["args"]["retry_after_s"], mcphost::state::SIGNUP_RATE_LIMIT_WINDOW_SECS);

    // The follow-up call is accepted by the tool's schema (no unknown_argument).
    let follow = McpClient::new(&server.base_url)
        .tools_call("host.quickstart", next["args"].clone())
        .await;
    assert!(follow.is_ok(), "{follow:?}");
}
