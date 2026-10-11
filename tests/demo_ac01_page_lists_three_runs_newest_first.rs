//! PRD-mcphost-demo-url-per-tool
//! AC1 (P0) — Given a url-shared tool with 3 recorded runs, When
//! `GET /d/<ns>/<tool>` is served, Then it is 200 HTML with exactly 3 rows
//! newest first, each showing status, `duration_ms`, an args excerpt and a
//! result excerpt each ≤ 200 chars, and `host.tool_logs(tool)` gains one
//! `demo_view` line.

use crate::common;
use common::{TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn demo_page_lists_three_runs_newest_first_and_logs_a_view() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Demo AC1").await;
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
    for text in ["first-run", "second-run", "third-run"] {
        let resp = http
            .post(&url)
            .header("content-type", "application/json")
            .body(json!({"text": text}).to_string())
            .send()
            .await
            .expect("POST /x");
        assert_eq!(resp.status(), 200);
    }

    let resp = http
        .get(format!("{}/d/{}/echo", server.base_url, ns))
        .send()
        .await
        .expect("GET /d");
    assert_eq!(resp.status(), 200);
    let ct = resp.headers()["content-type"].to_str().unwrap().to_string();
    assert!(ct.starts_with("text/html"), "content-type {ct}");
    let html = resp.text().await.unwrap();

    let rows: Vec<&str> = html.split("<tr class=\"run\">").skip(1).collect();
    assert_eq!(rows.len(), 3, "exactly 3 rows: {html}");
    // Newest first: the third run's args appear in the first row.
    assert!(rows[0].contains("third-run"), "row 0: {}", rows[0]);
    assert!(rows[1].contains("second-run"), "row 1: {}", rows[1]);
    assert!(rows[2].contains("first-run"), "row 2: {}", rows[2]);
    for row in &rows {
        assert!(row.contains("<td>done</td>"), "status shown: {row}");
        assert!(row.contains(" ms</td>"), "duration_ms shown: {row}");
        let codes: Vec<&str> = row
            .split("<code>")
            .skip(1)
            .map(|c| c.split("</code>").next().unwrap())
            .collect();
        assert_eq!(codes.len(), 2, "args + result excerpts: {row}");
        for c in codes {
            assert!(!c.is_empty() && c.chars().count() <= 200, "excerpt len: {c}");
        }
    }

    let logs = extract_structured(
        &client
            .tools_call("host.tool_logs", json!({"name": "echo"}))
            .await
            .expect("tool_logs"),
    );
    let views = logs["lines"]
        .as_array()
        .expect("lines")
        .iter()
        .filter(|l| l.as_str().is_some_and(|s| s.contains("demo_view")))
        .count();
    assert_eq!(views, 1, "one demo_view line: {logs}");
}
