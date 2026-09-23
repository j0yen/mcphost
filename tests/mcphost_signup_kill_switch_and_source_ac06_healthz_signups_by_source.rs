//! AC6 (PRD-mcphost-signup-kill-switch-and-source) — Given three external
//! signups with sources `hn`, `hn`, `reddit` and one synthetic signup,
//! When admin healthz is read, Then `signups_by_source.external` is
//! `{"hn": 2, "reddit": 1}` and `signups_enabled` is true.
//!
//! Same seam `attrib_ac4_external_signup_counts_real.rs` documents:
//! `TestServer`'s listener is always reached over loopback, so an
//! HTTP-level `signup` call can never classify as `external` -- this test
//! calls `control::signup` directly with a fake external IP for the three
//! real signups instead.

use crate::common;
use common::{ADMIN_KEY, TestServer};
use mcphost::control::{SignupAttribution, signup};
use serde_json::json;

async fn healthz(base_url: &str) -> serde_json::Value {
    reqwest::Client::new()
        .get(format!("{base_url}/healthz"))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .expect("GET /healthz")
        .json()
        .await
        .expect("parse /healthz")
}

#[tokio::test]
async fn healthz_breaks_down_external_signups_by_source() {
    let server = TestServer::start().await;

    signup(
        &server.state,
        &json!({"name": "HN One", "source": "hn"}),
        "8.8.8.8",
        SignupAttribution::default(),
    )
    .await
    .expect("signup hn 1");
    signup(
        &server.state,
        &json!({"name": "HN Two", "source": "hn"}),
        "8.8.8.8",
        SignupAttribution::default(),
    )
    .await
    .expect("signup hn 2");
    signup(
        &server.state,
        &json!({"name": "Reddit One", "source": "reddit"}),
        "8.8.8.8",
        SignupAttribution::default(),
    )
    .await
    .expect("signup reddit");
    signup(
        &server.state,
        &json!({"name": "Synthetic Agent"}),
        "8.8.8.8",
        SignupAttribution {
            synthetic_header: Some("synthorg:run-ac6"),
            ..Default::default()
        },
    )
    .await
    .expect("synthetic signup");

    let health = healthz(&server.base_url).await;
    assert_eq!(
        health["signups_by_source"]["external"],
        json!({"hn": 2, "reddit": 1}),
        "{health:?}"
    );
    assert_eq!(health["signups_enabled"], json!(true), "{health:?}");
}
