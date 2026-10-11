//! PRD-mcphost-blob-put-get-signed-url
//! AC3 — Given that blob's `url`, When the signature is altered or `expires`
//! is in the past, Then the route answers 403 and 410 respectively, and the
//! same test body passes unchanged against `/exports/{id}` and `/charts/{id}`.

use crate::common;
use base64::Engine as _;
use common::{McpClient, TestServer, extract_structured, signup};
use reqwest::StatusCode;
use serde_json::json;

use crate::chart_fixture;

/// The one test body (R5). Given a route's live signed URL, its path, the
/// query key it reads its expiry from and the owning tenant's key hash:
/// - the live URL answers 200;
/// - an altered signature answers 403;
/// - a missing signature answers 403;
/// - a genuine signature over a past `expires` answers 410.
async fn assert_route_contract(
    base_url: &str,
    key_hash: &str,
    path: &str,
    expires_param: &str,
    live_url: &str,
) {
    let live = reqwest::get(live_url).await.expect("GET live url");
    assert_eq!(live.status(), StatusCode::OK, "live url: {live_url}");

    let flipped = if live_url.ends_with('0') { '1' } else { '0' };
    let altered = format!("{}{flipped}", &live_url[..live_url.len() - 1]);
    let resp = reqwest::get(&altered).await.expect("GET altered url");
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "altered: {altered}");

    let unsigned = format!("{base_url}{path}");
    let resp = reqwest::get(&unsigned).await.expect("GET unsigned url");
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "unsigned: {unsigned}");

    let past = mcphost::state::now_unix() - 10;
    let expired = mcphost::signed_url::mint(base_url, key_hash, path, expires_param, past);
    let resp = reqwest::get(&expired).await.expect("GET expired url");
    assert_eq!(resp.status(), StatusCode::GONE, "expired: {expired}");
}

async fn key_hash_of(server: &TestServer, ns: String) -> String {
    server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("find tenant")
        .expect("tenant exists")
        .key_hash
}

#[tokio::test]
async fn blob_route_answers_403_for_altered_and_410_for_expired() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Blobput AC3 Blob").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let put = extract_structured(
        &client
            .tools_call(
                "host.blob.put",
                json!({"name": "a.txt", "content_base64": base64::engine::general_purpose::STANDARD.encode(b"hello")}),
            )
            .await
            .expect("host.blob.put"),
    );
    let id = put["id"].as_str().unwrap();
    let key_hash = key_hash_of(&server, ns).await;
    assert_route_contract(
        &server.base_url,
        &key_hash,
        &format!("/blobs/{id}"),
        "expires",
        put["url"].as_str().unwrap(),
    )
    .await;
}

#[tokio::test]
async fn export_route_answers_403_for_altered_and_410_for_expired() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Blobput AC3 Export").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let enqueue = extract_structured(&client.tools_call("host.export", json!({})).await.expect("host.export"));
    let run_id = enqueue["run_id"].as_str().expect("run_id").to_string();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let url = loop {
        let got = extract_structured(
            &client.tools_call("host.runs.get", json!({"run_id": run_id})).await.expect("host.runs.get"),
        );
        if got["status"] == json!("done") {
            break got["result"]["download_url"].as_str().expect("download_url").to_string();
        }
        assert!(std::time::Instant::now() < deadline, "export never finished: {got:?}");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    let key_hash = key_hash_of(&server, ns).await;
    assert_route_contract(&server.base_url, &key_hash, &format!("/exports/{run_id}"), "expires", &url).await;
}

#[tokio::test]
async fn chart_route_answers_403_for_altered_and_410_for_expired() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Blobput AC3 Chart").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    chart_fixture::seed_expenses(&client).await;
    let chart = extract_structured(
        &client
            .tools_call(
                "host.table.chart",
                json!({
                    "sql": "SELECT category, SUM(amount) AS total FROM expenses GROUP BY category",
                    "share": true,
                }),
            )
            .await
            .expect("host.table.chart"),
    );
    let id = chart["chart_id"].as_str().expect("chart_id");
    let url = chart["share_url"].as_str().expect("share_url");
    let key_hash = key_hash_of(&server, ns).await;
    assert_route_contract(&server.base_url, &key_hash, &format!("/charts/{id}"), "exp", url).await;
}

#[test]
fn the_three_routes_mint_through_the_one_signed_url_module() {
    let key = "k";
    let expires = 1_900_000_000;
    assert_eq!(
        mcphost::export::signed_download_url("http://h", key, "r1", expires),
        mcphost::signed_url::mint("http://h", key, "/exports/r1", "expires", expires),
    );
    assert_eq!(
        mcphost::chart::signed_chart_url("http://h", key, "c1", expires),
        mcphost::signed_url::mint("http://h", key, "/charts/c1", "exp", expires),
    );
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for file in ["src/export.rs", "src/chart.rs", "src/blobs.rs"] {
        let text = std::fs::read_to_string(root.join(file)).expect("read source");
        assert!(text.contains("signed_url::"), "{file} must use signed_url::");
    }
}
