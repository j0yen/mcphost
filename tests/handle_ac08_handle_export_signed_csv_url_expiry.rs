//! PRD-mcphost-result-handles
//! AC8 (P1) -- Given a handle, When `host.table.handle_export` is called,
//! Then a signed `/exports/{run_id}` URL returns CSV with `row_count` data
//! rows plus a header within 24h and 410 after.

use std::time::{Duration, Instant};

use reqwest::StatusCode;
use serde_json::json;

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};

async fn wait_for_done(client: &McpClient, run_id: &str) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let got = extract_structured(
            &client.tools_call("host.runs.get", json!({"run_id": run_id})).await.expect("host.runs.get ok"),
        );
        if got["status"] == json!("done") || got["status"] == json!("error") {
            return got;
        }
        assert!(Instant::now() < deadline, "handle_export never finished: {got:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn handle_export_signed_url_downloads_csv_within_24h_then_410_after() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "ResultHandles AC8 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.table.create", json!({"name": "events", "columns": {"n": "integer"}}))
        .await
        .expect("create events");
    client
        .tools_call(
            "host.table.append",
            json!({"table": "events", "rows": (0..5).map(|n| json!({"n": n})).collect::<Vec<_>>()}),
        )
        .await
        .expect("append 5 rows");

    let materialize = extract_structured(
        &client
            .tools_call("host.table.query", json!({"sql": "SELECT n FROM events", "handle": true}))
            .await
            .expect("materialize"),
    );
    let handle = materialize["handle"].as_str().expect("handle").to_string();
    assert_eq!(materialize["row_count"], 5);

    let enqueue = extract_structured(
        &client
            .tools_call("host.table.handle_export", json!({"handle": handle}))
            .await
            .expect("host.table.handle_export ok"),
    );
    let run_id = enqueue["run_id"].as_str().expect("run_id field").to_string();
    let finished = wait_for_done(&client, &run_id).await;
    assert_eq!(finished["status"], json!("done"), "handle_export failed: {finished:?}");
    let download_url = finished["result"]["download_url"].as_str().expect("download_url field").to_string();
    assert_eq!(finished["result"]["row_count"], 5, "result: {finished}");

    // Within 24h: the real URL the run result carries downloads a CSV
    // with a header row plus exactly `row_count` data rows.
    let resp = reqwest::get(&download_url).await.expect("GET download_url");
    assert_eq!(resp.status(), StatusCode::OK, "url: {download_url}");
    assert_eq!(
        resp.headers().get("content-type").and_then(|v| v.to_str().ok()),
        Some("text/csv"),
        "content-type"
    );
    let body = resp.text().await.expect("csv body");
    let lines: Vec<&str> = body.lines().collect();
    assert_eq!(lines.len(), 6, "1 header + 5 data rows: {body:?}");
    assert_eq!(lines[0], "n", "header row: {body:?}");

    // After 24h: a validly-signed but expired URL, hand-crafted the same
    // way `export::download` verifies one -- no real 24h wait needed since
    // the expiry is embedded in the URL itself, not tracked server-side
    // (same convention `mcphost_tenant_data_export_ac02`'s own AC2 test
    // uses for the tar.gz export's signed URL).
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("find tenant").expect("tenant exists");
    let past_expiry = mcphost::state::now_unix() - 10;
    let expired_url = mcphost::export::signed_download_url(&server.base_url, &tenant.key_hash, &run_id, past_expiry);
    let resp = reqwest::get(&expired_url).await.expect("GET expired url");
    assert_eq!(resp.status(), StatusCode::GONE, "url: {expired_url}");
}
