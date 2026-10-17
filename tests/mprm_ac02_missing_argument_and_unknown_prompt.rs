//! AC2 (P0) -- PRD-mcphost-prompts-from-published-tools. A known recipe with
//! a required argument missing answers `-32602` with `data.argument` naming
//! it; an unknown prompt name answers `-32602 prompt_not_found`.

use crate::common;
use common::{McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn mprm_ac02_missing_argument_and_unknown_prompt() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);
    client.initialize().await;

    let missing = client.rpc("prompts/get", json!({"name": "schedules", "arguments": {"tool": "t"}})).await;
    assert_eq!(missing["error"]["code"], -32602, "{missing}");
    assert_eq!(missing["error"]["data"]["argument"], "schedule", "{missing}");

    let none = client.rpc("prompts/get", json!({"name": "schedules"})).await;
    assert_eq!(none["error"]["code"], -32602, "{none}");
    assert_eq!(none["error"]["data"]["argument"], "tool", "first missing required argument: {none}");

    let unknown = client.rpc("prompts/get", json!({"name": "no-such-prompt"})).await;
    assert_eq!(unknown["error"]["code"], -32602, "{unknown}");
    assert_eq!(unknown["error"]["data"]["code"], "prompt_not_found", "{unknown}");
    assert!(unknown["error"]["message"].as_str().unwrap_or_default().contains("prompt_not_found"), "{unknown}");
}
