//! PRD-mcphost-capabilities-generated-from-dispatch
//! AC5 (P0) -- Given `GET /capabilities.json` keyless, When fetched, Then
//! `primitives.elicitation.server_to_client == ["elicitation/create"]`,
//! `primitives.roots.server_to_client == ["roots/list"]`,
//! `primitives.sampling.server_to_client == ["sampling/createMessage"]`, every
//! `methods` value is still an array of strings, and `elicitation/create`
//! POSTed by a client answers `-32601` with `data.code ==
//! "method_server_to_client"` and a `help_url`.

use crate::common;
use common::TestServer;
use serde_json::{Value, json};

#[tokio::test]
async fn cgd_ac05_capabilities_json_lists_server_to_client_and_the_client_post_is_refused() {
    let server = TestServer::start().await;
    let doc: Value = reqwest::get(format!("{}/capabilities.json", server.base_url))
        .await
        .expect("GET /capabilities.json")
        .json()
        .await
        .expect("JSON");
    let prim = &doc["primitives"];
    assert_eq!(prim["elicitation"]["server_to_client"], json!(["elicitation/create"]), "{doc}");
    assert_eq!(prim["roots"]["server_to_client"], json!(["roots/list"]), "{doc}");
    assert_eq!(prim["sampling"]["server_to_client"], json!(["sampling/createMessage"]), "{doc}");
    for (name, row) in prim.as_object().expect("primitives object") {
        let methods = row["methods"].as_array().unwrap_or_else(|| panic!("`{name}`.methods is not an array: {row}"));
        assert!(methods.iter().all(Value::is_string), "`{name}`.methods holds a non-string: {row}");
    }

    let reply = common::raw_rpc(&server.base_url, "/mcp", "elicitation/create", json!({}), false).await;
    let err = &reply["error"];
    assert_eq!(err["code"], -32601, "reply: {reply}");
    assert_eq!(err["data"]["code"], "method_server_to_client", "reply: {reply}");
    assert!(
        err["data"]["help_url"].as_str().unwrap_or_default().ends_with("/help/method_server_to_client"),
        "reply: {reply}"
    );
}
