//! AC8 — Given a keyed tenant on `build_router` with the prod transport, When
//! it calls `initialize`, Then the result advertises `tools.listChanged` true,
//! `prompts.listChanged` true and `resources.listChanged` false, and every
//! `list_changed: true` capability row names a `proving_cell`.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::capabilities::CAPABILITIES;
use serde_json::json;

#[tokio::test]
async fn sublis_ac08_initialize_advertises_tools_and_prompts_list_changes_and_names_the_proof() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "sublis8").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let init = client.initialize().await;
    let caps = &init["result"]["capabilities"];
    assert_eq!(caps["tools"]["listChanged"], json!(true), "{init}");
    assert_eq!(caps["prompts"]["listChanged"], json!(true), "{init}");
    assert!(
        caps["resources"]["listChanged"].is_null() || caps["resources"]["listChanged"] == json!(false),
        "resources.listChanged is not advertised: {init}"
    );

    let advertising: Vec<&str> = CAPABILITIES.iter().filter(|c| c.list_changed).map(|c| c.name).collect();
    assert_eq!(advertising, ["tools", "prompts"], "only tools and prompts advertise list changes");
    for row in CAPABILITIES.iter().filter(|c| c.list_changed) {
        assert_eq!(row.proving_cell, Some("subscriptions_listen"), "row `{}` names its proving cell", row.name);
    }

    // /capabilities.json exports the cell, so the conformance link can read its cell list from prod.
    let doc: serde_json::Value =
        reqwest::get(format!("{}/capabilities.json", server.base_url)).await.expect("get").json().await.expect("json");
    for name in ["tools", "prompts"] {
        assert_eq!(doc["primitives"][name]["proving_cell"], json!("subscriptions_listen"), "{doc}");
    }
    assert_eq!(doc["primitives"]["resources"]["list_changed"], json!(false), "{doc}");
}
