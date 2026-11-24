//! PRD-mcphost-implicit-signup-key-in-hand
//! AC1 (P0) -- Given the in-process test host and a client that never echoes
//! `Mcp-Session-Id`, When its first `host.table.create` with no `tenant_key`
//! runs, Then the result carries `implicit: true` and `onboarding.tenant_key`
//! equal to the minted tenant's key, `onboarding.note` names that field and
//! the `tenant_key` argument, and the same first call from a client that
//! presented its initialized session carries no such field.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use mcphost::auth::hash_key;
use serde_json::json;

const UA: &str = "kih-ac01/agent";

fn create_args() -> serde_json::Value {
    json!({"name": "runs", "columns": {"id": "integer"}})
}

#[tokio::test]
async fn kih_ac01() {
    let server = TestServer::start().await;

    // No `initialize`, no `Mcp-Session-Id` echoed: the claude-agent-sdk shape.
    let sessionless = McpClient::new(&server.base_url).with_user_agent(UA);
    let created = extract_structured(
        &sessionless.tools_call("host.table.create", create_args()).await.expect("first keyless call is an implicit signup"),
    );
    assert_eq!(created["implicit"], json!(true), "{created}");
    let onboarding = &created["onboarding"];
    let namespace = onboarding["tenant"].as_str().expect("onboarding.tenant");
    let key = onboarding["tenant_key"].as_str().expect("onboarding.tenant_key is the minted tenant's key");
    let tenant = server
        .state
        .db
        .find_tenant_by_key_hash(hash_key(key))
        .await
        .expect("lookup")
        .expect("the handed key is the minted tenant's key");
    assert_eq!(tenant.namespace, namespace, "{onboarding}");
    let note = onboarding["note"].as_str().expect("note");
    assert!(note.contains("onboarding.tenant_key"), "{note}");
    assert!(note.contains("tenant_key argument"), "{note}");
    assert!(!note.contains(key), "the note never carries the key itself: {note}");
    let memory_line = onboarding["memory_line"].as_str().unwrap_or_default();
    assert!(!memory_line.contains(key), "memory_line never carries the key: {memory_line}");

    // A client that presented its initialized session is served by the binding.
    let presenting = McpClient::new(&server.base_url).with_user_agent("kih-ac01/presenting").with_session_continuity();
    presenting.initialize().await;
    assert!(presenting.session_id().is_some(), "initialize must hand out a session id");
    let created = extract_structured(
        &presenting.tools_call("host.table.create", create_args()).await.expect("presenting client's first call"),
    );
    let onboarding = &created["onboarding"];
    assert!(onboarding["tenant"].as_str().is_some(), "still an implicit signup: {created}");
    assert!(onboarding.get("tenant_key").is_none(), "no key in a session-presenting envelope: {onboarding}");
    assert!(created.get("implicit").is_none(), "{created}");
    assert!(!onboarding["note"].as_str().expect("note").contains("onboarding.tenant_key"), "today's note text is kept");
}
