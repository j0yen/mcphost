//! PRD-mcphost-email-send-live-proof
//! AC6 (P0) -- Given the fake email client answering 503 twice, When `send`
//! runs on a verified tenant, Then the `host.email.sends` row reads
//! `provider_error`, stdout's verdict line is `status=provider_error
//! error=<the row's error verbatim>`, the exit code is 1, and
//! `offboarded=true` is still printed.

use crate::common;
use common::{ADMIN_KEY, TestServer};
use mcphost::email::FakeEmailClient;
use std::sync::Arc;

#[tokio::test]
async fn emslp_ac06_a_503_twice_turns_the_proof_red_with_the_rows_error() {
    let fake = Arc::new(FakeEmailClient::new());
    fake.fail_next_with_status(2, 503);
    let server = TestServer::start_with_email(fake.clone()).await;
    let url = server.base_url.clone();
    let out = tokio::task::spawn_blocking(move || {
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/email-probe.sh");
        std::process::Command::new("bash")
            .arg(script)
            .args(["send", "--verify", "admin"])
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
    assert_eq!(out.status.code(), Some(1), "{stdout}{}", String::from_utf8_lossy(&out.stderr));

    let lines: Vec<&str> = stdout.lines().collect();
    let ns = lines[0].trim_start_matches("tenant=");
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("open raw db");
    let (status, error): (String, Option<String>) = conn
        .query_row(
            "SELECT s.status, s.error FROM email_sends s JOIN tenants t ON t.id = s.tenant_id WHERE t.namespace = ?1",
            [ns],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("exactly one email_sends row");
    assert_eq!(status, "provider_error");
    let error = error.expect("the row records the provider status");
    assert!(error.contains("503"), "{error}");
    assert_eq!(lines[2], format!("status=provider_error error={error}"), "{stdout}");
    assert_eq!(lines.last().copied(), Some("offboarded=true"), "{stdout}");
    assert_eq!(fake.send_count(), 0, "nothing was accepted");
}
