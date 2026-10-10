//! PRD-mcphost-contract-version-reported AC5 — Given anonymous
//! `GET /contract.json`, When fetched, Then it is 200 with `ETag` equal to
//! the quoted sha, `Cache-Control: max-age=300`, a body whose `tools` length
//! equals `tools/list`'s host surface, and `contract_sha`; Given
//! `If-None-Match` with that ETag, Then 304 with an empty body.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::Value;

#[tokio::test]
async fn contract_json_serves_etag_cache_control_and_304() {
    let server = TestServer::start().await;
    let http = reqwest::Client::new();
    let url = format!("{}/contract.json", server.base_url);

    let resp = http.get(&url).send().await.expect("GET /contract.json");
    assert_eq!(resp.status(), 200);
    let sha = server.state.contract.sha.clone();
    let etag = format!("\"{sha}\"");
    assert_eq!(resp.headers()["etag"].to_str().unwrap(), etag);
    assert_eq!(resp.headers()["cache-control"].to_str().unwrap(), "max-age=300");
    let body: Value = resp.json().await.expect("json body");
    assert_eq!(body["contract_sha"], sha.as_str());

    let (_ns, key) = signup(&server.base_url, "CTRV AC5").await;
    let listed = McpClient::with_bearer(&server.base_url, &key)
        .tools_list()
        .await
        .expect("tools/list");
    let host_surface = listed["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .filter(|t| {
            t["name"]
                .as_str()
                .is_some_and(|n| n.starts_with("host") || n.starts_with("billing"))
        })
        .count();
    assert_eq!(
        body["tools"].as_array().expect("tools").len(),
        host_surface,
        "/contract.json tools must equal tools/list's host*/billing* surface (dotted names and their underscore aliases)"
    );

    let cached = http
        .get(&url)
        .header("If-None-Match", &etag)
        .send()
        .await
        .expect("conditional GET");
    assert_eq!(cached.status(), 304);
    assert_eq!(cached.headers()["etag"].to_str().unwrap(), etag);
    assert!(cached.bytes().await.expect("body").is_empty());
}
