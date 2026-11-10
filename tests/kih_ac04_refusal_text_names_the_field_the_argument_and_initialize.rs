//! PRD-mcphost-implicit-signup-key-in-hand
//! AC4 (P0) -- Given the refusal of AC 3, When its `message` and `data.hint`
//! are read, Then they name `onboarding.tenant_key`, the `tenant_key`
//! argument and `initialize`, name neither signup's key nor another tenant's
//! namespace, `errors.json` (the `error_registry` generator) lists that text
//! under `tenant_key_missing.reasons`, and the `ipsess` hint assertions are
//! revised to this wording in the same change.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use mcphost::errors::{PEER_RECENT_HINT, PEER_RECENT_MESSAGE};
use serde_json::{Value, json};

const UA: &str = "kih-ac04/agent";

fn create_args() -> Value {
    json!({"name": "runs", "columns": {"id": "integer"}})
}

#[tokio::test]
async fn kih_ac04() {
    let server = TestServer::start().await;
    server.state.redeemed_peers.set_cap(1);

    let minted = extract_structured(
        &McpClient::new(&server.base_url)
            .with_user_agent(UA)
            .tools_call("host.table.create", create_args())
            .await
            .expect("the first sessionless call mints"),
    );
    let namespace = minted["onboarding"]["tenant"].as_str().expect("onboarding.tenant").to_string();
    let key = minted["onboarding"]["tenant_key"].as_str().expect("onboarding.tenant_key").to_string();

    let err = McpClient::new(&server.base_url)
        .with_user_agent(UA)
        .tools_call("host.table.create", create_args())
        .await
        .expect_err("the second sessionless call is over the cap of 1");
    assert_eq!(err.data["reason"], json!("peer_recent"), "{err:?}");
    let hint = err.data["hint"].as_str().expect("data.hint");
    assert_eq!(hint, PEER_RECENT_HINT);
    assert_eq!(err.message, PEER_RECENT_MESSAGE, "the wire message is the constant the registry lists");
    for (label, text) in [("message", err.message.as_str()), ("hint", hint)] {
        for needle in ["onboarding.tenant_key", "tenant_key argument", "initialize"] {
            assert!(text.contains(needle), "{label} names {needle}: {text}");
        }
        assert!(!text.contains("signup"), "{label} does not point at signup's key: {text}");
        assert!(!text.contains(&namespace), "{label} names no tenant: {text}");
        assert!(!text.contains(&key), "{label} carries no key: {text}");
    }

    let body: Value = reqwest::Client::new()
        .get(format!("{}/errors.json", server.base_url))
        .send()
        .await
        .expect("GET /errors.json")
        .json()
        .await
        .expect("errors.json is JSON");
    let row = body["codes"]
        .as_array()
        .expect("codes")
        .iter()
        .find(|r| r["code"] == "tenant_key_missing")
        .expect("tenant_key_missing row");
    let listed = row["reasons"]
        .as_array()
        .expect("tenant_key_missing.reasons")
        .iter()
        .find(|r| r["reason"] == "peer_recent")
        .expect("peer_recent reason row")["hint"]
        .as_str()
        .expect("hint")
        .to_string();
    assert_eq!(listed, hint, "errors.json lists the text the refusal carries");
}
