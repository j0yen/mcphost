//! PRD-mcphost-ownership-moment
//! AC6 (P0) — Given the journey in test mode, When the claim step runs,
//! Then it completes the magic-link page through the fake mail client and
//! the step is P0 (a failure fails the journey).
//!
//! The journey harness itself (`synthorg journey run`'s `claim` step,
//! promoted from an always-skip P1 stub to a real P0 round trip, and
//! `mcphost-deploy`'s `journey.py` reading that step's verdict off its own
//! `claim` key to decide a rollback) lives in the `synthorg`/
//! `mcphost-deploy` repos -- landed there directly (synthorg branch
//! `wm-build/mcphost-ownership-moment-journey-claim-p0`, same branch name
//! in `mcphost-deploy`). This file pins the one contract on the mcphost
//! side that step actually depends on: `signup`'s `claim_url`, `POST
//! /claim/{token}` (form `email`), the magic-link token extractable from
//! the fake mail client's own captured message by the exact
//! `/claim/verify/([A-Za-z0-9_-]+)` pattern the journey step's own
//! extractor uses, `GET /claim/verify/{code}`, and `host.whoami`'s
//! `owner_verified` flipping true -- the same sequence
//! `synthorg.journey._claim_result` performs over real HTTP.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;
use std::sync::Arc;

fn token_from_verify_url(body: &str) -> &str {
    let idx = body.find("/claim/verify/").expect("verify URL in email body");
    let after = &body[idx + "/claim/verify/".len()..];
    after
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .next()
        .expect("token token")
}

#[tokio::test]
async fn the_magic_link_round_trip_the_journey_claim_step_performs_completes() {
    let fake = Arc::new(mcphost::email::FakeEmailClient::new());
    let server = TestServer::start_with_email(fake.clone()).await;
    let anon = McpClient::new(&server.base_url);

    // signup's claim_url -- the journey step's own starting point.
    let raw = anon
        .tools_call("signup", json!({"name": "AC6 Journey Tenant"}))
        .await
        .expect("signup");
    let result = extract_structured(&raw);
    let key = result["key"].as_str().expect("key").to_string();
    let claim_url = result["claim_url"].as_str().expect("claim_url").to_string();
    let token = claim_url.rsplit('/').next().expect("claim token").to_string();

    // claim_start: POST /claim/{token} with an email, form-encoded.
    let http = reqwest::Client::new();
    let resp = http
        .post(format!("{}/claim/{token}", server.base_url))
        .form(&[("email", "journey@example.org")])
        .send()
        .await
        .expect("POST /claim/{token}");
    assert!(resp.status().is_success(), "claim_start must succeed: {}", resp.status());

    // read_mail_sink + magic-link extraction: the fake client's own
    // captured message must carry a /claim/verify/<token> URL.
    let sends = fake.sends();
    assert_eq!(sends.len(), 1, "exactly one magic-link email expected");
    let verify_token = token_from_verify_url(&sends[0].text_body);

    // claim_verify: GET /claim/verify/{code}.
    let verify_resp = http
        .get(format!("{}/claim/verify/{verify_token}", server.base_url))
        .send()
        .await
        .expect("GET /claim/verify/{code}");
    assert!(verify_resp.status().is_success(), "claim_verify must succeed: {}", verify_resp.status());

    // host.whoami.owner_verified: the journey step's own completion check.
    let tenant_client = McpClient::with_bearer(&server.base_url, &key);
    let whoami = extract_structured(
        &tenant_client
            .tools_call("host.whoami", json!({}))
            .await
            .expect("host.whoami after claim"),
    );
    assert_eq!(whoami["owner_verified"], json!(true), "{whoami}");
}
