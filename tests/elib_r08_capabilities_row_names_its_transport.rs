//! PRD-mcphost-elicitation-in-band-on-stateless-transport R8 — the
//! `/capabilities.json` row `elicitation` carries `transport`, rendered from
//! `capabilities::transport_note` (one source), and no other row does.

use crate::common;
use common::TestServer;
use serde_json::Value;

#[tokio::test]
async fn only_the_elicitation_row_names_its_transport() {
    let server = TestServer::start().await;
    let doc: Value = reqwest::get(format!("{}/capabilities.json", server.base_url))
        .await
        .expect("get")
        .json()
        .await
        .expect("json");
    let expected = mcphost::capabilities::transport_note("elicitation").expect("elicitation has a transport note");
    assert_eq!(doc["primitives"]["elicitation"]["transport"].as_str(), Some(expected), "{doc}");
    assert!(expected.contains("in-band") && expected.contains("Mcp-Session-Id") && expected.contains("link"), "{expected}");
    for (name, row) in doc["primitives"].as_object().expect("primitives") {
        assert_eq!(row.get("transport").is_some(), name == "elicitation", "row `{name}`: {row}");
    }
}
