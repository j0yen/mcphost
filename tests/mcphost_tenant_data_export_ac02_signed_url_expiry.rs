//! PRD-mcphost-tenant-data-export
//! AC2 (P0) -- Given a completed export, When the signed URL is fetched
//! within 24 h, Then the archive downloads; after 24 h, Then 410.

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
        assert!(Instant::now() < deadline, "export never finished: {got:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn signed_url_downloads_within_24h_then_410_after() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Export AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let enqueue = extract_structured(
        &client
            .tools_call("host.export", json!({}))
            .await
            .expect("host.export ok"),
    );
    let run_id = enqueue["run_id"].as_str().expect("run_id field").to_string();
    let finished = wait_for_done(&client, &run_id).await;
    assert_eq!(finished["status"], json!("done"), "export failed: {finished:?}");
    let download_url = finished["result"]["download_url"]
        .as_str()
        .expect("download_url field")
        .to_string();

    // Within 24h: the real URL the run result carries downloads.
    let resp = reqwest::get(&download_url).await.expect("GET download_url");
    assert_eq!(resp.status(), StatusCode::OK, "url: {download_url}");
    let body = resp.bytes().await.expect("archive body");
    // gzip magic bytes.
    assert_eq!(&body[..2], &[0x1f, 0x8b], "not a gzip archive");

    // After 24h: a validly-signed but expired URL, hand-crafted the same
    // way `export::download` verifies one -- no real 24h wait needed since
    // the expiry is embedded in the URL itself, not tracked server-side.
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
