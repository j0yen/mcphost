//! PRD-mcphost-first-call-gift
//! AC4 — Given the same session as AC3, When a second `host.*` call
//! succeeds, Then its result carries no `welcome_back`.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn second_call_of_the_new_session_carries_no_welcome_back() {
    let server = TestServer::start().await;

    let session1 = McpClient::new(&server.base_url).with_session_continuity();
    session1
        .tools_call("signup", json!({"name": "AC4 Tenant", "remember": "cleaning Joe's CSVs"}))
        .await
        .expect("signup with remember");
    session1
        .tools_call("host.whoami", json!({}))
        .await
        .expect("host.whoami on the earlier session");
    let rotated = extract_structured(
        &session1
            .tools_call("host.key_rotate", json!({}))
            .await
            .expect("host.key_rotate mints a personal URL"),
    );
    let url = rotated["url"].as_str().expect("url").to_string();
    let path = url.trim_start_matches(&server.base_url).to_string();

    let session2 = McpClient::new(&server.base_url).with_path(&path).with_session_continuity();
    let first_call = extract_structured(
        &session2
            .tools_call("host.whoami", json!({}))
            .await
            .expect("first call of the new URL-bound session"),
    );
    assert!(
        first_call.get("welcome_back").is_some(),
        "the first call of the new session must carry welcome_back: {first_call}"
    );

    let second_call = extract_structured(
        &session2
            .tools_call("host.whoami", json!({}))
            .await
            .expect("second call of the same new URL-bound session"),
    );
    assert!(
        second_call.get("welcome_back").is_none(),
        "a second call on the same session must carry no welcome_back: {second_call}"
    );
}
