//! PRD-mcphost-demo-url-per-tool
//! AC5 (P0) — Given 25 recorded runs, When the page renders, Then only 10
//! rows appear, the response is ≤ 64 KB, and the headers carry
//! `X-Robots-Tag: noindex` and `Cache-Control: no-store`.

use crate::common;
use common::{TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn page_shows_ten_of_twenty_five_runs_within_64kb_and_is_noindex() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Demo AC5").await;
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
    let big = "<&>\"".repeat(500);
    for i in 0..25 {
        let resp = http
            .post(&url)
            .header("content-type", "application/json")
            .body(json!({"text": format!("run-{i:02}-{big}")}).to_string())
            .send()
            .await
            .expect("POST /x");
        assert_eq!(resp.status(), 200);
    }

    let resp = http.get(format!("{}/d/{}/echo", server.base_url, ns)).send().await.expect("GET /d");
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers()["x-robots-tag"], "noindex");
    assert_eq!(resp.headers()["cache-control"], "no-store");
    let bytes = resp.bytes().await.unwrap();
    assert!(bytes.len() <= 64 * 1024, "page is {} bytes", bytes.len());
    let html = String::from_utf8(bytes.to_vec()).unwrap();
    assert_eq!(html.matches("<tr class=\"run\">").count(), 10, "exactly 10 rows");
    assert!(html.contains("run-24-"), "newest run shown");
    assert!(!html.contains("run-14-"), "11th-newest run not shown");
}
