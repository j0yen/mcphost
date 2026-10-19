//! PRD-mcphost-email-send-live-proof
//! AC2 (P0) -- Given the in-process server with `FakeEmailClient` and the
//! probe tenant's owner verified, When `send --to delivered@resend.dev` runs,
//! Then the fake recorded one message from `agent@<slug>.<send_domain>` to
//! that address, `host.email.sends` lists a `sent` row with a
//! `provider_message_id`, stdout's verdict line is `status=sent id=em_<ulid>
//! provider_message_id=<id>` followed by `offboarded=true`, and the exit code
//! is 0.

use crate::common;
use common::{ADMIN_KEY, TestServer};
use mcphost::email::FakeEmailClient;
use std::sync::Arc;

#[tokio::test]
async fn emslp_ac02_send_prints_the_sent_row_and_offboards() {
    let fake = Arc::new(FakeEmailClient::new());
    let server = TestServer::start_with_email(fake.clone()).await;
    let url = server.base_url.clone();
    let out = tokio::task::spawn_blocking(move || {
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/email-probe.sh");
        std::process::Command::new("bash")
            .arg(script)
            .args(["send", "--to", "delivered@resend.dev", "--verify", "admin"])
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("MCPHOST_URL", url)
            .env("MCPHOST_ADMIN_KEY", ADMIN_KEY)
            .output()
            .expect("run script")
    })
    .await
    .expect("join");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stdout}{stderr}");

    let lines: Vec<&str> = stdout.lines().collect();
    assert!(lines[0].starts_with("tenant="), "{stdout}");
    let ns = lines[0].trim_start_matches("tenant=");
    assert_eq!(lines[1], "verify=admin", "{stdout}");
    assert_eq!(lines.last().copied(), Some("offboarded=true"), "{stdout}");

    // The sent row prod recorded, read straight from SQLite after the run.
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("open raw db");
    let (id, status, pmid): (String, String, Option<String>) = conn
        .query_row(
            "SELECT s.id, s.status, s.provider_message_id FROM email_sends s \
             JOIN tenants t ON t.id = s.tenant_id WHERE t.namespace = ?1",
            [ns],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .expect("exactly one email_sends row");
    assert_eq!(status, "sent");
    let pmid = pmid.expect("provider_message_id");
    assert!(id.starts_with("em_") && id.len() > 3, "{id}");
    assert_eq!(lines[2], format!("status=sent id={id} provider_message_id={pmid}"), "{stdout}");

    let sent = fake.sends();
    assert_eq!(sent.len(), 1, "{sent:?}");
    let slug = ns.to_ascii_lowercase().replace('_', "-");
    assert_eq!(sent[0].from, format!("agent@{slug}.{}", mcphost::email_send::send_domain()));
    assert_eq!(sent[0].to, "delivered@resend.dev");
}
