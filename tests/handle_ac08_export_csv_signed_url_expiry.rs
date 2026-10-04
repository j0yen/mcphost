//! PRD-mcphost-result-handles
//! AC8 -- Given a handle, When `host.table.handle_export` is called, Then
//! a signed `/exports/{run_id}` URL returns CSV with `row_count` data
//! rows plus a header within 24 h and 410 after.

use std::time::{Duration, Instant};

use reqwest::StatusCode;
use serde_json::json;

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};

async fn wait_for_done(client: &McpClient, run_id: &str) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let got = extract_structured(
            &client
                .tools_call("host.runs.get", json!({"run_id": run_id}))
                .await
                .expect("host.runs.get ok"),
        );
        if got["status"] == json!("done") || got["status"] == json!("error") {
            return got;
        }
        assert!(Instant::now() < deadline, "handle export never finished: {got:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn handle_export_csv_downloads_within_24h_then_410_after() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Handle AC8 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({"name": "readings", "columns": {"sensor": "text", "value": "real"}}),
        )
        .await
        .expect("create readings");
    let rows: Vec<_> = (0..40)
        .map(|i| json!({"sensor": format!("s{i}"), "value": i as f64}))
        .collect();
    client
        .tools_call("host.table.append", json!({"table": "readings", "rows": rows}))
        .await
        .expect("append readings");

    let summary = extract_structured(
        &client
            .tools_call("host.table.query", json!({"sql": "SELECT * FROM readings", "handle": true}))
            .await
            .expect("materialize handle"),
    );
    let handle = summary["handle"].as_str().expect("handle name").to_string();
    assert_eq!(summary["row_count"], 40, "summary: {summary}");

    let enqueue = extract_structured(
        &client
            .tools_call("host.table.handle_export", json!({"handle": handle}))
            .await
            .expect("host.table.handle_export ok"),
    );
    let run_id = enqueue["run_id"].as_str().expect("run_id field").to_string();
    let finished = wait_for_done(&client, &run_id).await;
    assert_eq!(finished["status"], json!("done"), "handle export failed: {finished:?}");
    assert_eq!(finished["result"]["row_count"], 40, "result: {finished}");
    let download_url = finished["result"]["download_url"]
        .as_str()
        .expect("download_url field")
        .to_string();

    // Within 24h: the real URL the run result carries downloads exactly
    // row_count data rows plus a header.
    let resp = reqwest::get(&download_url).await.expect("GET download_url");
    assert_eq!(resp.status(), StatusCode::OK, "url: {download_url}");
    assert_eq!(
        resp.headers().get("content-type").and_then(|v| v.to_str().ok()),
        Some("text/csv")
    );
    let body = resp.text().await.expect("csv body");
    let lines: Vec<&str> = body.lines().collect();
    assert_eq!(lines.len(), 41, "expected 40 data rows + 1 header, got {}: {body}", lines.len());
    assert_eq!(lines[0], "sensor,value", "header: {}", lines[0]);

    // After 24h: a validly-signed but expired URL, hand-crafted the same
    // way `export::download` verifies one.
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("find tenant")
        .expect("tenant exists");
    let past_expiry = mcphost::state::now_unix() - 10;
    let expired_url =
        mcphost::export::signed_download_url(&server.base_url, &tenant.key_hash, &run_id, past_expiry);
    let resp = reqwest::get(&expired_url).await.expect("GET expired url");
    assert_eq!(resp.status(), StatusCode::GONE, "url: {expired_url}");
}
