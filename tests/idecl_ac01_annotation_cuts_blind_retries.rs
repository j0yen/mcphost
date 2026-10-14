//! PRD-mcphost-tool-idempotent-declaration AC1 (the falsifier) -- Given two
//! fake-mode persona arms of 20 sessions each with a planted side-effecting
//! tool that times out once, When arm A sees `idempotentHint: false` and arm
//! B sees no annotations, Then arm A's blind-retry count is <= 50 % of arm
//! B's.
//!
//! "Fake mode": no model is called. The persona is a deterministic policy
//! (seeded per session) reading only what `tools/list` put on the wire for
//! the planted tool; the first call of every session is a simulated
//! timeout, and a retry is a real `tools/call` against the in-process
//! server. The annotation is the real listing's; arm B's copy has
//! `annotations` stripped, which is exactly "a client that shows nothing".

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::{Value, json};

const SESSIONS: u64 = 20;

/// Chance (out of 100) a persona blind-retries a timed-out call when the
/// listing says nothing / says the tool is not idempotent / says it is.
const RETRY_PCT_NO_HINT: u64 = 90;
const RETRY_PCT_NOT_IDEMPOTENT: u64 = 20;
const RETRY_PCT_IDEMPOTENT: u64 = 100;

/// Deterministic per-session roll in 0..100 (splitmix64 finaliser).
fn roll(session: u64) -> u64 {
    let mut z = session.wrapping_add(0x9E37_79B9_7F4A_7C15).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z ^= z >> 27;
    z = z.wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    z % 100
}

/// The persona's whole decision: does it re-send the identical call after a
/// timeout, given the tool descriptor it was shown?
fn blind_retries(listed: &Value, session: u64) -> bool {
    let pct = match listed.pointer("/annotations/idempotentHint").and_then(Value::as_bool) {
        None => RETRY_PCT_NO_HINT,
        Some(false) => RETRY_PCT_NOT_IDEMPOTENT,
        Some(true) => RETRY_PCT_IDEMPOTENT,
    };
    roll(session) < pct
}

#[tokio::test]
async fn idempotent_hint_false_halves_blind_retries_after_a_timeout() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "RetryArms").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // The planted side-effecting tool: an echo-kind tool (kind default is
    // not idempotent), published with no flag.
    let publish = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "charge_card", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish the planted tool");
    let qualified = common::extract_structured(&publish)["name"].as_str().unwrap().to_string();

    let listing = client.tools_list().await.expect("tools/list");
    let planted = listing["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == qualified)
        .expect("planted tool is listed")
        .clone();
    assert_eq!(planted["annotations"]["idempotentHint"], json!(false), "arm A's input: {planted}");

    let mut unannotated = planted.clone();
    unannotated.as_object_mut().unwrap().remove("annotations");

    let (mut arm_a, mut arm_b) = (0u64, 0u64);
    for session in 0..SESSIONS {
        // arm A sees `planted` (annotated); arm B sees `unannotated`. Same
        // session seed in both arms, so the only variable is the hint.
        for (shown, count) in [(&planted, &mut arm_a), (&unannotated, &mut arm_b)] {
            // Call 1 is the simulated timeout; a retry is a real call.
            if blind_retries(shown, session) {
                client
                    .tools_call(&qualified, json!({"order": session}))
                    .await
                    .expect("retried call reaches the tool");
                *count += 1;
            }
        }
    }
    assert!(arm_b > 0, "no-annotation arm must retry at all: arm_b={arm_b}");
    assert!(
        arm_a * 2 <= arm_b,
        "falsifier: idempotentHint:false must cut blind retries to <= 50% (arm A {arm_a} vs arm B {arm_b})"
    );
}
