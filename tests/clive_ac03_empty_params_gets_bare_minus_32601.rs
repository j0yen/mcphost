//! AC3 (P1) -- PRD-mcphost-server-capabilities-live-proof, INVERTED by
//! PRD-mcphost-capabilities-generated-from-dispatch (AC7). The pinned body
//! with `params` replaced by `{}` used to answer a bare `-32601` with no
//! `data` member: rmcp 3.x parses the request into a `ClientRequest` BEFORE
//! dispatch, `{}` cannot deserialize as a `CompleteRequestParam`, so the
//! transport answered its own `method_not_found` and no hook ever ran. The
//! pre-dispatch gate now reads `method` from the body first, so the same
//! request answers `data.code == "capability_unsupported"`. The file keeps its
//! old name so the AC's pointers in agent/ stay valid.

use crate::common;
use common::TestServer;
use serde_json::Value;

const FIXTURE: &str = include_str!("fixtures/clive_complete_keyless.json");

#[tokio::test]
async fn pinned_body_with_empty_params_gets_labelled_minus_32601() {
    let mut body: Value = serde_json::from_str(FIXTURE).expect("fixture is JSON");
    body["params"] = serde_json::json!({});
    let server = TestServer::start().await;
    let reply: Value = reqwest::Client::new()
        .post(format!("{}/mcp", server.base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(body.to_string())
        .send()
        .await
        .expect("POST /mcp")
        .json()
        .await
        .expect("JSON reply");
    assert_eq!(reply["error"]["code"], -32601, "reply: {reply}");
    assert_eq!(reply["error"]["data"]["code"], "capability_unsupported", "reply: {reply}");
}
