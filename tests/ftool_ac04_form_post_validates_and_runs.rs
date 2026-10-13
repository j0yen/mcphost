//! PRD-mcphost-form-tool
//! AC4 (P0) — Given that page, When the form is POSTed with `amount=abc`,
//! Then it re-renders with the `args_invalid` text beside `amount` and no run
//! row exists; When POSTed with valid values, Then a run row with
//! `trigger=form` exists and the page shows `success_text`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "amount": {"type": "number"},
            "category": {"type": "string", "enum": ["travel", "meals"]},
            "note": {"type": "string"},
        },
        "required": ["note"],
    })
}

async fn setup(label: &str) -> (TestServer, McpClient, String) {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, label).await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "expense_submit", "kind": "echo",
                   "spec": {"schema": schema(), "ui": {"form": true, "success_text": "Recorded, thanks"}}}),
        )
        .await
        .expect("publish with ui.form");
    let shared = extract_structured(
        &client
            .tools_call("host.tool_share", json!({"name": "expense_submit", "visibility": "url"}))
            .await
            .expect("share"),
    );
    let form_url = shared["form_url"].as_str().expect("form_url").to_string();
    (server, client, form_url)
}

fn csrf_of(html: &str) -> String {
    html.split("name=\"_csrf\" value=\"").nth(1).and_then(|r| r.split('"').next()).expect("hidden csrf field").to_string()
}

async fn runs(client: &McpClient) -> Vec<Value> {
    let listed = extract_structured(&client.tools_call("host.runs.list", json!({})).await.expect("runs.list"));
    listed["runs"].as_array().cloned().unwrap_or_default()
}

#[tokio::test]
async fn invalid_amount_rerenders_with_error_beside_field_and_creates_no_run() {
    let (_server, client, form_url) = setup("Ftool AC4 invalid").await;
    let http = reqwest::Client::new();
    let page = http.get(&form_url).send().await.unwrap().text().await.unwrap();
    let csrf = csrf_of(&page);

    let resp = http
        .post(&form_url)
        .form(&[("amount", "abc"), ("category", "travel"), ("note", "taxi"), ("_csrf", csrf.as_str())])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let html = resp.text().await.unwrap();
    let after_amount = html.split("name=\"amount\"").nth(1).expect("amount field rendered");
    let before_next = after_amount.split("<label").next().unwrap();
    assert!(before_next.contains("class=\"error\""), "error div sits beside amount: {before_next}");
    assert!(before_next.contains("args.amount: expected number, got string"), "args_invalid text: {before_next}");
    assert!(html.contains("value=\"abc\"") || html.contains("abc"), "submitted value is kept");
    assert!(!html.contains("Recorded, thanks"), "no success text on failure");
    assert!(runs(&client).await.is_empty(), "a failed validation creates no run row");
}

#[tokio::test]
async fn valid_post_creates_form_run_shows_success_text_and_notifies_inbox() {
    let (_server, client, form_url) = setup("Ftool AC4 valid").await;
    let http = reqwest::Client::new();
    let page = http.get(&form_url).send().await.unwrap().text().await.unwrap();
    let csrf = csrf_of(&page);

    let resp = http
        .post(&form_url)
        .form(&[("amount", "12.5"), ("category", "meals"), ("note", "lunch"), ("_csrf", csrf.as_str())])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let html = resp.text().await.unwrap();
    assert!(html.contains("Recorded, thanks"), "success_text shown: {html}");

    let rows = runs(&client).await;
    assert_eq!(rows.len(), 1, "exactly one run: {rows:?}");
    assert_eq!(rows[0]["trigger"], json!("form"), "{rows:?}");
    let run_id = rows[0]["run_id"].as_str().or_else(|| rows[0]["id"].as_str()).expect("run id").to_string();

    let inbox = extract_structured(&client.tools_call("host.msg.inbox", json!({})).await.expect("inbox"));
    let messages = inbox["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 1, "one inbox message: {inbox}");
    let data = &messages[0]["data"];
    assert_eq!(data["kind"], json!("form_submission"), "{inbox}");
    assert_eq!(data["tool"], json!("expense_submit"));
    assert_eq!(data["run_id"], json!(run_id));
    assert_eq!(data["args"], json!({"amount": 12.5, "category": "meals", "note": "lunch"}));
    assert!(data["ip_hash"].is_string());
}

#[tokio::test]
async fn post_without_a_valid_csrf_token_is_refused_and_runs_nothing() {
    let (_server, client, form_url) = setup("Ftool AC4 csrf").await;
    let resp = reqwest::Client::new()
        .post(&form_url)
        .form(&[("note", "x"), ("_csrf", "1.deadbeef")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);
    assert!(runs(&client).await.is_empty());
}
