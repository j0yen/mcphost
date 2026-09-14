//! PRD-mcphost-agent-inbox
//! Regression — advisory finding `dedupe-resend-refused-list-not-reconstructed`
//! (gate review, 2026-09-14): `Db::dedupe_hit` used to return
//! `refused: Vec::new()` unconditionally on a `(sender, dedupe_key)` cache
//! hit, so a resend of an originally-mixed delivered/refused send silently
//! reported everyone delivered. This reproduces that exact shape: one
//! recipient delivered, one recipient refused (nonexistent address), same
//! `dedupe_key` sent twice.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn regr01_dedupe_hit_replays_original_mixed_delivered_and_refused_result() {
    let server = TestServer::start().await;
    let (_ns_a, key_a) = signup(&server.base_url, "Agent A").await;
    let (ns_b, _key_b) = signup(&server.base_url, "Agent B").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);

    let to = json!([ns_b.clone(), "t_doesnotexist00000000"]);

    let first_raw = client_a
        .tools_call(
            "host.msg.send",
            json!({"to": to.clone(), "body": "ping", "dedupe_key": "regr01"}),
        )
        .await
        .expect("first send: one delivered, one refused");
    let first = extract_structured(&first_raw);
    assert_eq!(first["delivered_to"], json!([ns_b]), "{first:?}");
    let first_refused = first["refused"].as_array().expect("refused array");
    assert_eq!(first_refused.len(), 1, "{first:?}");
    assert_eq!(first_refused[0]["address"], json!("t_doesnotexist00000000"), "{first:?}");
    assert_eq!(first_refused[0]["code"], json!("agent_not_found"), "{first:?}");

    // Resend with the same dedupe_key: requirement 8 says this must return
    // the *original* message_id and store nothing new -- it must also
    // replay the *original* full outcome, not just delivered_to.
    let resend_raw = client_a
        .tools_call(
            "host.msg.send",
            json!({"to": to, "body": "ping", "dedupe_key": "regr01"}),
        )
        .await
        .expect("resend with same dedupe_key hits the cache");
    let resend = extract_structured(&resend_raw);

    assert_eq!(resend["message_id"], first["message_id"], "{resend:?}");
    assert_eq!(resend["delivered_to"], first["delivered_to"], "{resend:?}");
    let resend_refused = resend["refused"].as_array().expect("refused array");
    assert_eq!(
        resend_refused, first_refused,
        "dedupe-hit resend must report the same refused list as the original send, {resend:?}"
    );
}
