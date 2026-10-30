//! PRD-mcphost-implicit-signup-per-session-not-per-peer
//! AC4 (P0) -- Given every `Reason` variant, When each error's `message` and
//! `data.hint` are rendered, Then the PeerRecent and ConnectionBound texts
//! contain neither `pass the key` nor `tenant_key argument`, the Anonymous
//! text is byte-identical to v0.118.8, and `errors.json` (the `error_registry`
//! generator) lists the three hints under `tenant_key_missing.reasons`.

use crate::common::TestServer;
use mcphost::errors::{AppError, Reason, connection_hint};
use serde_json::{Value, json};

#[test]
fn ipsess_ac04_texts_name_a_session_or_a_tenant_url_never_the_key() {
    let anonymous = AppError::TenantKeyMissing(Reason::Anonymous).into_error_data();
    assert_eq!(
        anonymous.message.as_ref(),
        "tenant_key is required: pass the key that signup returned as the tenant_key argument",
        "the Anonymous text is byte-identical to v0.118.8"
    );
    assert!(anonymous.data.as_ref().is_none_or(|d| d.get("hint").is_none()), "{anonymous:?}");

    for reason in [Reason::ConnectionBound("ns_probe".to_string()), Reason::PeerRecent] {
        let wire = reason.wire();
        let err = AppError::TenantKeyMissing(reason).into_error_data();
        let hint = err.data.as_ref().and_then(|d| d["hint"].as_str()).unwrap_or_default().to_string();
        assert!(!hint.is_empty(), "{wire}: a hint is rendered");
        for (label, text) in [("message", err.message.to_string()), ("hint", hint)] {
            assert!(!text.contains("pass the key"), "{wire} {label}: {text}");
            assert!(!text.contains("tenant_key argument"), "{wire} {label}: {text}");
            assert!(text.contains("tenant URL") || text.contains("/t/"), "{wire} {label} names a remedy: {text}");
        }
    }
}

#[tokio::test]
async fn ipsess_ac04() {
    let server = TestServer::start().await;
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
    let reasons = row["reasons"].as_array().expect("tenant_key_missing.reasons");
    let wires: Vec<&str> = reasons.iter().map(|r| r["reason"].as_str().expect("reason")).collect();
    assert_eq!(wires, Reason::ALL_WIRE.to_vec(), "one row per wire reason, in ALL_WIRE order");

    let hint_of = |wire: &str| reasons.iter().find(|r| r["reason"] == wire).expect("row")["hint"].as_str().expect("hint");
    assert_eq!(hint_of("missing_key"), AppError::TenantKeyMissing(Reason::Anonymous).to_string());
    assert_eq!(hint_of("connection_bound"), connection_hint("<namespace>"));
    assert_eq!(hint_of("peer_recent"), mcphost::errors::PEER_RECENT_HINT);
    for wire in ["connection_bound", "peer_recent"] {
        assert!(!hint_of(wire).contains("pass the key"), "{wire}");
        assert!(!hint_of(wire).contains("tenant_key argument"), "{wire}");
    }
    assert_eq!(json!(reasons.len()), json!(3));
}
