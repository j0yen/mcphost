//! AC1 (P0) -- PRD-mcphost-server-capabilities-live-proof. The pinned request
//! body `tests/fixtures/clive_complete_keyless.json` is POSTed keyless to the
//! in-process `/mcp` and answers `-32601` carrying the `capability_unsupported`
//! envelope. The same file is what the Live AC sends to prod, and
//! `creg_ac03::params_for` reads its `completion/complete` params from it.

use crate::common;
use common::TestServer;
use serde_json::Value;

const FIXTURE: &str = include_str!("fixtures/clive_complete_keyless.json");

#[tokio::test]
async fn pinned_complete_body_keyless_gets_capability_unsupported() {
    let server = TestServer::start().await;
    let reply: Value = reqwest::Client::new()
        .post(format!("{}/mcp", server.base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(FIXTURE.as_bytes().to_vec())
        .send()
        .await
        .expect("POST /mcp")
        .json()
        .await
        .expect("JSON reply");
    let error = &reply["error"];
    assert_eq!(error["code"], -32601, "reply: {reply}");
    assert_eq!(error["data"]["code"], "capability_unsupported", "reply: {reply}");
    assert_eq!(error["data"]["capability"], "completions", "reply: {reply}");
    let help_url = error["data"]["help_url"].as_str().unwrap_or_default();
    assert!(help_url.ends_with("/help/capability_unsupported"), "help_url {help_url:?} in {reply}");
}
