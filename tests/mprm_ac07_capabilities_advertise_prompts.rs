//! AC7 (P0) -- PRD-mcphost-prompts-from-published-tools. `initialize`
//! answers `capabilities.prompts == {listChanged: true}` and
//! `/capabilities.json` reports `prompts.implemented == true`, both from the
//! one `prompts` row of `capabilities.rs`.

use crate::common;
use common::{McpClient, TestServer};
use mcphost::capabilities::CAPABILITIES;
use serde_json::{Value, json};

#[tokio::test]
async fn mprm_ac07_capabilities_advertise_prompts() {
    let row = CAPABILITIES.iter().find(|c| c.name == "prompts").expect("prompts row");
    assert!(row.implemented && row.list_changed, "the registry row is the source of both answers");

    let server = TestServer::start().await;
    let init = McpClient::new(&server.base_url).initialize().await;
    assert_eq!(init["result"]["capabilities"]["prompts"], json!({"listChanged": true}), "{init}");

    let doc: Value = reqwest::get(format!("{}/capabilities.json", server.base_url))
        .await
        .expect("get")
        .json()
        .await
        .expect("json");
    assert_eq!(doc["primitives"]["prompts"]["implemented"], true, "{doc}");
    assert_eq!(doc["primitives"]["prompts"]["list_changed"], true, "{doc}");
}
