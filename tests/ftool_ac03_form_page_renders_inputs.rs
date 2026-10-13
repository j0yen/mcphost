//! PRD-mcphost-form-tool
//! AC3 (P0) — Given a url-shared tool with `ui.form` and three properties
//! (number, enum, required string), When `GET /f/<token>/<tool>` is fetched
//! with no key, Then the page has a number input, a select with the enum
//! values, and a required text input, and contains no source or tenant key
//! bytes.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

pub fn expense_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "amount": {"type": "number", "description": "How much, in dollars"},
            "category": {"type": "string", "enum": ["travel", "meals"]},
            "note": {"type": "string"},
        },
        "required": ["note"],
    })
}

#[tokio::test]
async fn get_form_page_renders_one_input_per_property_and_leaks_nothing() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Ftool AC3").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "expense_submit", "kind": "echo", "spec": {"schema": expense_schema(), "ui": {"form": true}}}),
        )
        .await
        .expect("publish with ui.form");
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "other_hidden_tool", "kind": "echo", "spec": {"schema": expense_schema()}}),
        )
        .await
        .expect("publish a second tool");
    let shared = extract_structured(
        &client
            .tools_call("host.tool_share", json!({"name": "expense_submit", "visibility": "url"}))
            .await
            .expect("share by url"),
    );
    let form_url = shared["form_url"].as_str().expect("share response carries form_url").to_string();
    assert!(form_url.contains("/f/") && form_url.ends_with("/expense_submit"), "form_url: {form_url}");

    let resp = reqwest::Client::new().get(&form_url).send().await.expect("GET form page");
    assert_eq!(resp.status(), 200);
    let content_type = resp.headers()["content-type"].to_str().unwrap().to_string();
    assert!(content_type.starts_with("text/html"), "content-type: {content_type}");
    let html = resp.text().await.unwrap();

    assert!(html.contains(r#"<input type="number" name="amount""#), "number input: {html}");
    assert!(html.contains(r#"<select name="category""#), "select: {html}");
    assert!(html.contains(r#"<option value="travel">travel</option>"#), "enum option travel: {html}");
    assert!(html.contains(r#"<option value="meals">meals</option>"#), "enum option meals: {html}");
    let note = html.split("<input type=\"text\" name=\"note\"").nth(1).expect("text input for note");
    let note_tag = note.split('>').next().unwrap();
    assert!(note_tag.contains(" required"), "note input is required: {note_tag}");
    assert!(html.contains("How much, in dollars"), "description shown as help text");

    assert!(!html.contains(&key), "page must not contain the tenant key");
    assert!(!html.contains("other_hidden_tool"), "page must not name other tools");
    assert!(!html.contains("\"schema\""), "page must not carry the spec: {html}");
}

#[tokio::test]
async fn tool_without_ui_form_has_no_form_page_and_no_form_url() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Ftool AC3 plain").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call("host.tool_publish", json!({"name": "plain", "kind": "echo", "spec": {"schema": expense_schema()}}))
        .await
        .expect("publish");
    let shared = extract_structured(
        &client.tools_call("host.tool_share", json!({"name": "plain", "visibility": "url"})).await.expect("share"),
    );
    assert!(shared.get("form_url").is_none(), "no form_url without ui.form: {shared}");
    let token_url = shared["url"].as_str().unwrap().replace("/x/", "/f/");
    let resp = reqwest::Client::new().get(&token_url).send().await.unwrap();
    assert_eq!(resp.status(), 404);
    assert!(resp.bytes().await.unwrap().is_empty(), "404 bytes are empty");
}
