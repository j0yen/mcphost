//! AC3 (P1) -- PRD-mcphost-server-capabilities-live-proof. The pinned body
//! with `params` replaced by `{}` answers a bare `-32601` with no `data`
//! member. Reason: rmcp 3.2.0 parses the request into a `ClientRequest`
//! BEFORE dispatch; `{}` cannot deserialize as a `CompleteRequestParam`, so
//! the transport answers its own `method_not_found` and
//! `ServerHandler::complete` (which builds the `capability_unsupported`
//! envelope) never runs. Do not write an AC against this reply.

use crate::common;
use common::TestServer;
use serde_json::Value;

const FIXTURE: &str = include_str!("fixtures/clive_complete_keyless.json");

#[tokio::test]
async fn pinned_body_with_empty_params_gets_bare_minus_32601() {
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
    assert!(reply["error"].get("data").is_none(), "no data member expected, reply: {reply}");
}
