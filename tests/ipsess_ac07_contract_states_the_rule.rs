//! PRD-mcphost-implicit-signup-per-session-not-per-peer
//! AC7 (P1) -- Given the test host, When `/contract.json` is fetched
//! anonymously, Then `onboarding.implicit_signup.scope` is `"session"` and
//! `onboarding.peer_recent.window_secs` is `600`.

use crate::common::TestServer;
use serde_json::{Value, json};

#[tokio::test]
async fn ipsess_ac07() {
    let server = TestServer::start().await;
    let contract: Value = reqwest::Client::new()
        .get(format!("{}/contract.json", server.base_url))
        .send()
        .await
        .expect("GET /contract.json")
        .json()
        .await
        .expect("contract.json is JSON");
    assert_eq!(contract["onboarding"]["implicit_signup"]["scope"], json!("session"), "{}", contract["onboarding"]);
    assert_eq!(contract["onboarding"]["peer_recent"]["window_secs"], json!(600), "{}", contract["onboarding"]);
    assert_eq!(
        contract["onboarding"]["peer_recent"]["window_secs"],
        json!(mcphost::peer_recent::PEER_TTL_SECS),
        "generated from the gate's own constant"
    );
}
