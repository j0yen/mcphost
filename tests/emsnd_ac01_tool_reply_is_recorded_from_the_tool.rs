//! PRD-mcphost-send-email-from-tool
//! AC1 (P0) -- Given a task like synthorg's `email_forward_receipt` extended
//! with a reply step, When it runs in fake-provider mode, Then the send is
//! counted (`host.email.sends` lists it) and `FakeEmailClient` recorded a
//! message whose From is `<tool>@<tenant>.mcphost.dev`, so the falsifier
//! ("zero `host_email_send` calls from tools") is measurable.

use crate::common;
use common::{TestServer, extract_structured, poll_until_ready, signup};
use mcphost::email::FakeEmailClient;
use mcphost::sandbox;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn emsnd_ac01_a_tools_reply_is_sent_from_the_tool_and_counted() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let fake = Arc::new(FakeEmailClient::new());
    let server = TestServer::start_python_with_email(&envs_dir.0, fake.clone()).await;
    let (ns, key) = signup(&server.base_url, "emsnd ac01 tenant").await;
    let agent = common::McpClient::with_bearer(&server.base_url, &key);
    // The reply goes to the verified owner's address (AC5's anti-spam line).
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("open raw db");
    conn.execute(
        "UPDATE tenants SET owner_email = 'owner@example.com', owner_verified_at = 1700000000 WHERE namespace = ?1",
        rusqlite::params![ns],
    )
    .expect("verify owner");
    drop(conn);

    let spec = json!({
        "source": "import mcphost\n\ndef main(args):\n    return mcphost.email.send(\n        to=args[\"from\"], subject=\"Re: \" + args[\"subject\"], text=\"Receipt filed.\")\n",
    });
    agent
        .tools_call("host.tool_publish", json!({"name": "email_forward_receipt", "kind": "python", "spec": spec}))
        .await
        .expect("publish ok");

    let called = extract_structured(
        &poll_until_ready(
            &agent,
            &format!("{ns}.email_forward_receipt"),
            json!({"from": "owner@example.com", "subject": "Receipt 42"}),
            Duration::from_secs(15),
        )
        .await
        .unwrap_or_else(|e| panic!("tool call must succeed: {} {}", e.code, e.message)),
    );
    assert!(called["id"].as_str().is_some_and(|id| id.starts_with("em_")), "{called}");

    let sent = fake.sends();
    assert_eq!(sent.len(), 1, "exactly one provider send: {sent:?}");
    let slug = ns.to_ascii_lowercase().replace('_', "-");
    assert_eq!(sent[0].from, format!("email_forward_receipt@{slug}.mcphost.dev"));
    assert_eq!(sent[0].to, "owner@example.com");
    assert_eq!(sent[0].subject, "Re: Receipt 42");

    let listed = extract_structured(&agent.tools_call("host.email.sends", json!({"limit": 20})).await.expect("sends"));
    let rows = listed["sends"].as_array().expect("sends array");
    assert_eq!(rows.len(), 1, "{listed}");
    assert_eq!(rows[0]["tool"], json!("email_forward_receipt"), "{listed}");
    assert_eq!(rows[0]["status"], json!("sent"), "{listed}");
}
