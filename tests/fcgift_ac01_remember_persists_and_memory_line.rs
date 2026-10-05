//! PRD-mcphost-first-call-gift
//! AC1 — Given a fresh anonymous session, When `signup` is called with
//! `remember: "cleaning Joe's CSVs"`, Then the response carries
//! `remembered.key` starting `notes/`, `remembered.text` equal to the
//! input, `memory_line` of at most 200 characters containing the tenant's
//! URL or `host.whoami`, and `host.state.get` of that key under the new
//! tenant returns the same text.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn signup_with_remember_stores_a_note_and_returns_a_memory_line() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);
    let result = client
        .tools_call(
            "signup",
            json!({"name": "AC1 Tenant", "remember": "cleaning Joe's CSVs"}),
        )
        .await
        .expect("signup with remember");
    let signup = extract_structured(&result);

    let key = signup["key"].as_str().expect("key field").to_string();

    let note_key = signup["remembered"]["key"].as_str().expect("remembered.key");
    assert!(note_key.starts_with("notes/"), "{signup}");
    assert_eq!(signup["remembered"]["text"], json!("cleaning Joe's CSVs"), "{signup}");

    let memory_line = signup["memory_line"].as_str().expect("memory_line field");
    assert!(
        memory_line.len() <= 200,
        "memory_line must be at most 200 characters: {memory_line:?}"
    );
    assert!(
        memory_line.contains("host.whoami") || memory_line.contains(&server.base_url),
        "memory_line must contain the tenant's URL or host.whoami: {memory_line:?}"
    );
    assert!(signup["memory_hint"].as_str().is_some(), "{signup}");

    // `host.state.get` of that key under the new tenant returns the same text.
    let tenant_client = McpClient::with_bearer(&server.base_url, &key);
    let got = extract_structured(
        &tenant_client
            .tools_call("host.state.get", json!({"key": note_key}))
            .await
            .expect("host.state.get"),
    );
    assert_eq!(got["found"], json!(true), "{got}");
    assert_eq!(got["value"]["text"], json!("cleaning Joe's CSVs"), "{got}");
}
