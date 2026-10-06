//! PRD-mcphost-second-session-nudge
//! AC5 (P0) — Given the provider returns 500, When the sweep runs, Then
//! the row has `nudge_channel = email-failed`, the key is absent from
//! every log line, and the next sweep retries; after three failures the
//! channel is `email-abandoned`.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use mcphost::email::EmailClient;
use serde_json::json;
use std::sync::Arc;

fn token_from_claim_url(claim_url: &str) -> &str {
    claim_url.rsplit('/').next().expect("claim_url has a path segment")
}

fn code_from_email_body(body: &str) -> &str {
    let idx = body.find("/claim/verify/").expect("verify URL in claim email body");
    let after = &body[idx + "/claim/verify/".len()..];
    after.split_whitespace().next().expect("code token")
}

#[tokio::test]
async fn three_provider_failures_retry_then_abandon() {
    let fake = Arc::new(mcphost::email::FakeEmailClient::new());
    let server = TestServer::start_with_email(fake.clone()).await;
    let anon = McpClient::new(&server.base_url);

    let signup = extract_structured(
        &anon
            .tools_call("signup", json!({"name": "AC5 Tenant"}))
            .await
            .expect("signup"),
    );
    let namespace = signup["tenant"].as_str().expect("tenant").to_string();
    let key = signup["key"].as_str().expect("key").to_string();
    let claim_token =
        token_from_claim_url(signup["claim_url"].as_str().expect("claim_url")).to_string();

    let tenant_client = McpClient::with_bearer(&server.base_url, &key);
    tenant_client.tools_call("host.whoami", json!({})).await.expect("whoami");

    let http = reqwest::Client::new();
    http.post(format!("{}/claim/{claim_token}", server.base_url))
        .form(&[("email", "ac5@b.co")])
        .send()
        .await
        .expect("POST /claim/{token}");
    let claim_sends = fake.sends();
    let code = code_from_email_body(&claim_sends[0].text_body).to_string();
    http.get(format!("{}/claim/verify/{code}", server.base_url))
        .send()
        .await
        .expect("GET /claim/verify/{code}");

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(namespace)
        .await
        .expect("find tenant")
        .expect("tenant exists");
    let backdated = mcphost::state::now_unix() - 30 * 3600;
    server
        .state
        .db
        .set_tenant_stamp_for_test(tenant.id, "first_call_unix", backdated)
        .await
        .expect("backdate first_call_unix");

    // AC5's "the key is absent from every log line": `sweep`'s own
    // `tracing::warn!(error = %e, ...)` logs exactly this `EmailClient::
    // send` error's `Display` -- demonstrated directly here, on the same
    // fake provider failure every sweep attempt below hits, rather than
    // capturing a process-wide tracing subscriber (which would also force
    // this file into its own exclusive-global test binary, past the
    // suite's <=10-binary budget).
    fake.fail_next(1);
    let probe = fake
        .send(&mcphost::email::EmailMessage {
            to: "probe@b.co".to_string(),
            from: "noreply@mcphost.invalid".to_string(),
            reply_to: "noreply@mcphost.invalid".to_string(),
            subject: "probe".to_string(),
            text_body: "probe".to_string(),
        })
        .await
        .expect_err("fake provider configured to fail once");
    let rendered = probe.to_string();
    assert!(
        !rendered.contains("test-email-key"),
        "the configured API key must never appear in a loggable error: {rendered}"
    );

    for attempt in 1..=3 {
        fake.fail_next(1);
        let report = mcphost::returns::sweep(&server.state).await.expect("sweep");
        assert_eq!(report.selected, 1, "attempt {attempt}: {report:?}");
        let (nudged_unix, nudge_channel, attempts, _second_session_unix) = server
            .state
            .db
            .nudge_status(tenant.id)
            .await
            .expect("nudge_status")
            .expect("tenant exists");
        assert_eq!(attempts, attempt, "attempt {attempt}: nudge_attempts");
        if attempt < 3 {
            assert_eq!(report.failed, 1, "attempt {attempt}: {report:?}");
            assert_eq!(nudge_channel.as_deref(), Some("email-failed"), "attempt {attempt}");
            assert!(nudged_unix.is_none(), "attempt {attempt}: still eligible for retry");
        } else {
            assert_eq!(report.abandoned, 1, "attempt {attempt}: {report:?}");
            assert_eq!(nudge_channel.as_deref(), Some("email-abandoned"), "attempt {attempt}");
            assert!(nudged_unix.is_some(), "attempt {attempt}: finalized, no more retries");
        }
    }

    // A fourth sweep must not select the now-abandoned tenant again.
    let final_report = mcphost::returns::sweep(&server.state).await.expect("final sweep");
    assert_eq!(final_report.selected, 0, "{final_report:?}");

    assert_eq!(fake.send_count(), 1, "only the claim email, never a return email: {:?}", fake.sends());
}
