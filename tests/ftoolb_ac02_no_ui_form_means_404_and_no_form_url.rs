//! PRD-mcphost-form-tool-b
//! AC2 (P0) — Given a url-shared tool without `ui.form`, When
//! `/f/<token>/<tool>` is fetched, Then the 404 bytes are returned and
//! `host.tool_share`'s response has no `form_url`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn url_shared_tool_without_ui_form_has_no_form_page_and_no_form_url() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Ftoolb AC2").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "plain", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    let shared = extract_structured(
        &client.tools_call("host.tool_share", json!({"name": "plain", "visibility": "url"})).await.expect("share"),
    );
    assert!(shared.get("form_url").is_none(), "no form_url: {shared}");
    let url = shared["url"].as_str().expect("url");

    // The /f/ twin of the /x/ url a stranger could guess.
    let form_probe = url.replacen("/x/", "/f/", 1);
    let http = reqwest::Client::new();
    let resp = http.get(&form_probe).send().await.unwrap();
    assert_eq!(resp.status(), 404);
    assert!(resp.bytes().await.unwrap().is_empty(), "the 404 bytes are empty, as for an unknown token");

    // Identical to a token that never existed.
    let unknown = http.get(form_probe.replace(url.rsplit('/').nth(1).unwrap(), "nonexistenttoken")).send().await.unwrap();
    assert_eq!(unknown.status(), 404);
    assert!(unknown.bytes().await.unwrap().is_empty());
}
