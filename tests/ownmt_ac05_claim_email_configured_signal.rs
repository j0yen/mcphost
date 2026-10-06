//! PRD-mcphost-ownership-moment
//! AC5 (P0) — Given a host whose env contract requires mail and
//! `claim_email_configured = false`, When `mcphost-deploy probe` runs,
//! Then the artifact check fails with `claim-email-unconfigured` and
//! `doctor` names the missing variables; given the variables set, Then
//! the check passes.
//!
//! The artifact check and `doctor` wiring live in the `mcphost-deploy`
//! repo (python) -- landed and tested there directly (commit `41a8398`,
//! `tests/test_mail_env_ac1_fresh_and_redeploy_preserve.py`,
//! `test_mail_env_ac2_claim_email_configured.py`,
//! `test_mail_env_ac3_doctor_missing_keys.py`). This file pins the one
//! fact on the mcphost side that probe actually reads off the wire:
//! `/healthz`'s own `claim_email_configured` boolean, unconfigured by
//! default and true once all three `MCPHOST_EMAIL_*` fields are set.

use crate::common;
use common::{ADMIN_KEY, TestServer};
use mcphost::email::{EmailConfig, EmailProvider, FakeEmailClient};
use std::sync::Arc;

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
async fn unconfigured_by_default() {
    let server = TestServer::start().await;
    let body = healthz(&server.base_url).await;
    assert_eq!(body["claim_email_configured"], serde_json::json!(false));
}

#[tokio::test]
async fn configured_once_the_three_mail_variables_are_set() {
    let email_client: Arc<dyn mcphost::email::EmailClient> = Arc::new(FakeEmailClient::new());
    let email_config = EmailConfig {
        api_url: Some("https://email.invalid/send".to_string()),
        api_key: Some("test-email-key".to_string()),
        from: Some("claim@mcphost.dev".to_string()),
        provider: EmailProvider::default(),
    };
    let server = TestServer::start_full_with_email(
        Some(ADMIN_KEY.to_string()),
        mcphost::kinds::KindRegistry::with_builtin(),
        mcphost::state::CALL_TIMEOUT,
        None,
        mcphost::state::SIGNUP_RATE_LIMIT_PER_HOUR,
        mcphost::billing::BillingConfig::default(),
        Arc::new(mcphost::billing::FakeBillingClient::new(mcphost::state::now_unix())),
        Vec::new(),
        email_config,
        email_client,
        mcphost::state::CLAIM_TOKEN_TTL_SECS_DEFAULT,
        mcphost::state::CLAIM_RATE_LIMIT_PER_HOUR_DEFAULT,
        mcphost::db::DbConfig::from_env(),
        mcphost::alerts::AlertConfig::default(),
        mcphost::state::FleetIps::empty(),
        mcphost::state::VerifiedClientIds::empty(),
    )
    .await;

    let body = healthz(&server.base_url).await;
    assert_eq!(body["claim_email_configured"], serde_json::json!(true));
}
