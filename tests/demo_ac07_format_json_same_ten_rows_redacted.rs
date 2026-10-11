//! PRD-mcphost-demo-url-per-tool
//! AC7 (P1) — Given `?format=json`, When fetched, Then the body is a JSON
//! array of the same 10 rows with the same redaction and `Content-Type:
//! application/json`.

use crate::common;
use common::{TestServer, extract_structured, signup};
use serde_json::{Value, json};

#[tokio::test]
async fn format_json_returns_the_same_ten_redacted_rows() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Demo AC7").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "echo", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    let shared = extract_structured(
        &client
            .tools_call("host.tool_share", json!({"name": "echo", "visibility": "url"}))
            .await
            .expect("share"),
    );
    let url = shared["url"].as_str().expect("url").to_string();

    let http = reqwest::Client::new();
    for i in 0..12 {
        http.post(&url)
            .header("content-type", "application/json")
            .body(json!({"text": format!("run-{i:02}"), "api_key": "sk-PLANTED-API-KEY"}).to_string())
            .send()
            .await
            .expect("POST /x");
    }

    let page = format!("{}/d/{}/echo", server.base_url, ns);
    let html = http.get(&page).send().await.unwrap().text().await.unwrap();
    let resp = http.get(format!("{page}?format=json")).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers()["content-type"], "application/json");
    let body = resp.text().await.unwrap();
    assert!(!body.contains("sk-PLANTED-API-KEY"), "api_key value leaked: {body}");
    let rows: Vec<Value> = serde_json::from_str::<Value>(&body).unwrap().as_array().expect("array").clone();
    assert_eq!(rows.len(), 10);
    assert_eq!(html.matches("<tr class=\"run\">").count(), rows.len(), "same row count as the page");
    assert!(rows[0]["args"].as_str().unwrap().contains("run-11"), "newest first: {}", rows[0]);
    assert!(rows[0]["args"].as_str().unwrap().contains("•••"), "masked: {}", rows[0]);
    for r in &rows {
        assert_eq!(r["status"], json!("done"));
        assert!(r["duration_ms"].is_i64());
    }
}
