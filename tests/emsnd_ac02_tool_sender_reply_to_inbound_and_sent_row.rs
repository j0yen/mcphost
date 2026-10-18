//! PRD-mcphost-send-email-from-tool
//! AC2 (P0) -- Given a verified tenant and a tool run named `triage_mail`,
//! When the tool calls `mcphost.email.send`, Then the recorded message has
//! From `triage_mail@<slug>.mcphost.dev`, Reply-To the tenant's inbound
//! address, and a `sent` row with a `provider_message_id`.

use crate::common;
use common::{TestServer, extract_structured, poll_until_ready, signup};
use mcphost::email::FakeEmailClient;
use mcphost::sandbox;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

const OWNER: &str = "owner@example.com";

#[tokio::test]
async fn emsnd_ac02_tool_named_sender_with_inbound_reply_to_and_a_sent_row() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let fake = Arc::new(FakeEmailClient::new());
    let server = TestServer::start_python_with_email(&envs_dir.0, fake.clone()).await;
    let (ns, key) = signup(&server.base_url, "emsnd ac02 tenant").await;
    let agent = common::McpClient::with_bearer(&server.base_url, &key);

    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("open raw db");
    conn.execute(
        "UPDATE tenants SET owner_email = ?1, owner_verified_at = 1700000000 WHERE namespace = ?2",
        rusqlite::params![OWNER, ns],
    )
    .expect("verify owner");
    drop(conn);

    let spec = json!({
        "source": "import mcphost\n\ndef main(args):\n    return mcphost.email.send(args[\"to\"], \"Triaged\", \"Filed under billing.\")\n",
    });
    agent
        .tools_call("host.tool_publish", json!({"name": "triage_mail", "kind": "python", "spec": spec}))
        .await
        .expect("publish ok");
    poll_until_ready(&agent, &format!("{ns}.triage_mail"), json!({"to": OWNER}), Duration::from_secs(15))
        .await
        .unwrap_or_else(|e| panic!("tool call must succeed: {} {}", e.code, e.message));

    let slug = ns.to_ascii_lowercase().replace('_', "-");
    let sent = fake.sends();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].from, format!("triage_mail@{slug}.mcphost.dev"));
    assert_eq!(sent[0].reply_to, format!("inbox@{slug}.mcphost.dev"), "Reply-To is the tenant's inbound address");
    assert_eq!(sent[0].to, OWNER);

    let listed = extract_structured(&agent.tools_call("host.email.sends", json!({})).await.expect("sends"));
    let row = &listed["sends"][0];
    assert_eq!(row["status"], json!("sent"), "{listed}");
    assert!(row["provider_message_id"].as_str().is_some_and(|id| !id.is_empty()), "{listed}");
    assert_eq!(row["from"], json!(format!("triage_mail@{slug}.mcphost.dev")), "{listed}");
}
