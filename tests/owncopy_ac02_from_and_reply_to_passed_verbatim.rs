//! PRD-mcphost-ownership-copy
//! AC2 (P0) — Given `MCPHOST_EMAIL_FROM="mcphost <hello@mcphost.dev>"`,
//! When an email is sent through the fake provider, Then the request's
//! `from` is that exact string and `reply_to` is `hello@mcphost.dev`.
//!
//! Spawns the real `mcphost serve` binary
//! (`tests/busyaudit_ac02_env_override_busy_timeout.rs`'s subprocess
//! pattern) with `$MCPHOST_EMAIL_FROM` actually set in the child's
//! environment, rather than constructing an `EmailConfig` in-process:
//! `EmailConfig::from_env()` (src/email.rs) reads the *process*
//! environment and is only ever called from `main.rs`, so a subprocess is
//! the one way to exercise it for real without racing every other
//! in-process `TestServer` in this suite binary. The "fake provider" is a
//! local `wiremock::MockServer` standing in for the real email API at
//! `$MCPHOST_EMAIL_API_URL` -- the real `HttpEmailClient` posts to it, same
//! "tests never reach the network" guarantee the in-process
//! `FakeEmailClient` gives other email tests, just enforced at the HTTP
//! layer instead of the trait layer since a subprocess can't be handed a
//! Rust trait object.


use serde_json::json;
use crate::serve;
use serve::{ServeChild, ServeEnv};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::busyaudit::scratch_data_dir;
use crate::common::{McpClient, extract_structured};

fn token_from_claim_url(claim_url: &str) -> &str {
    claim_url.rsplit('/').next().expect("claim_url has a path segment")
}

#[tokio::test]
async fn from_is_passed_verbatim_and_reply_to_is_the_bare_address() {
    let fake_provider = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&fake_provider)
        .await;

    let data_dir = scratch_data_dir("owncopy-ac02");
    std::fs::create_dir_all(&data_dir).expect("create scratch data dir");
    let mut server = ServeChild::spawn(ServeEnv {
        data_dir: data_dir.clone(),
        extra_env: vec![
            ("MCPHOST_EMAIL_API_URL".into(), fake_provider.uri()),
            ("MCPHOST_EMAIL_FROM".into(), "mcphost <hello@mcphost.dev>".into()),
        ],
        ..ServeEnv::default()
    });
    server.wait_ready().await;
    let base_url = server.base_url();

    let anon = McpClient::new(&base_url);
    let raw = anon
        .tools_call("signup", json!({"name": "AC2 Tenant"}))
        .await
        .expect("signup");
    let result = extract_structured(&raw);
    let token = token_from_claim_url(result["claim_url"].as_str().expect("claim_url")).to_string();

    let http = reqwest::Client::new();
    let claim_resp = http
        .post(format!("{base_url}/claim/{token}"))
        .form(&[("email", "a@b.co")])
        .send()
        .await
        .expect("POST /claim/{token}");
    let claim_body = claim_resp.text().await.expect("claim response body");
    assert!(
        claim_body.contains("One more step"),
        "claim must succeed, not fall back to the send-error page: {claim_body}"
    );

    let requests = fake_provider
        .received_requests()
        .await
        .expect("wiremock must record requests");
    assert_eq!(requests.len(), 1, "exactly one ownership email sent to the provider");
    let body: serde_json::Value =
        serde_json::from_slice(&requests[0].body).expect("email send body is JSON");
    assert_eq!(body["from"], json!("mcphost <hello@mcphost.dev>"));
    assert_eq!(body["reply_to"], json!("hello@mcphost.dev"));

    drop(server);
    let _ = std::fs::remove_dir_all(&data_dir);
}
