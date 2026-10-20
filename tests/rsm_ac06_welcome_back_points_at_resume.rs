//! PRD-mcphost-resume-tool AC6 -- Given a URL-bound tenant with one note and
//! an earlier session, When the first `host.*` call of a new session
//! succeeds, Then `welcome_back.next` is `"host.resume"`, and a following
//! `host.resume` response carries no `welcome_back`.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

/// A tenant with one note and a finished earlier session; returns the path
/// of its personal URL.
async fn tenant_with_an_earlier_session(server: &TestServer, name: &str) -> String {
    let session1 = McpClient::new(&server.base_url).with_session_continuity();
    session1
        .tools_call("signup", json!({"name": name, "remember": "cleaning Joe's CSVs"}))
        .await
        .expect("signup with remember");
    session1.tools_call("host.whoami", json!({})).await.expect("host.whoami on the earlier session");
    let rotated =
        extract_structured(&session1.tools_call("host.key_rotate", json!({})).await.expect("host.key_rotate"));
    let url = rotated["url"].as_str().expect("url").to_string();
    url.trim_start_matches(&server.base_url).to_string()
}

#[tokio::test]
async fn welcome_back_names_host_resume_and_resume_itself_carries_none() {
    let server = TestServer::start().await;
    let path = tenant_with_an_earlier_session(&server, "RSM AC6").await;

    let session2 = McpClient::new(&server.base_url).with_path(&path).with_session_continuity();
    let first_call = extract_structured(&session2.tools_call("host.whoami", json!({})).await.expect("first call"));
    assert_eq!(first_call["welcome_back"]["next"], "host.resume", "{first_call}");

    let resume = extract_structured(&session2.tools_call("host.resume", json!({})).await.expect("host.resume"));
    assert!(resume.get("welcome_back").is_none(), "{resume}");
    assert!(resume.get("next").is_none(), "host.resume carries next_call, never a next hint: {resume}");
    assert!(resume.get("next_call").is_some(), "{resume}");
}

#[tokio::test]
async fn a_session_that_opens_with_host_resume_gets_no_welcome_back_and_no_repeat() {
    let server = TestServer::start().await;
    let path = tenant_with_an_earlier_session(&server, "RSM AC6 b").await;

    let session2 = McpClient::new(&server.base_url).with_path(&path).with_session_continuity();
    let resume = extract_structured(&session2.tools_call("host.resume", json!({})).await.expect("host.resume"));
    assert!(resume.get("welcome_back").is_none(), "{resume}");
    assert!(resume.get("next").is_none(), "{resume}");

    // The resume used up the session's welcome back.
    let next = extract_structured(&session2.tools_call("host.whoami", json!({})).await.expect("host.whoami"));
    assert!(next.get("welcome_back").is_none(), "{next}");
}
