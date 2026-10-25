//! PRD-mcphost-resource-listing-shape-in-contract
//! AC6 (P0) — Given an HTTP server started in a test with no tenant key,
//! When `GET /contract.json` is requested, Then the body's `resources`
//! object equals the dump's, the response carries the ETag the contract sha
//! defines, and a second request with `If-None-Match` gets 304.

use crate::common::TestServer;
use mcphost::api_contract::{dump_contract, dump_contract_bytes, sha_hex};
use mcphost::kinds::KindRegistry;
use serde_json::Value;

#[tokio::test]
async fn mcphost_resource_listing_shape_in_contract_ac06_anonymous_contract_json_serves_resources_etag_and_304() {
    let server = TestServer::start().await;
    let http = reqwest::Client::new();
    let url = format!("{}/contract.json", server.base_url);
    let kinds = KindRegistry::with_builtin();

    // No Authorization header: the route is anonymous.
    let resp = http.get(&url).send().await.expect("GET /contract.json");
    assert_eq!(resp.status(), 200);
    let etag = format!("\"{}\"", sha_hex(&dump_contract_bytes(&kinds)));
    assert_eq!(resp.headers()["etag"].to_str().unwrap(), etag, "ETag is the quoted contract sha");
    let body: Value = resp.json().await.expect("json body");
    let dumped = dump_contract(&kinds);
    assert!(body["resources"]["uri_scheme"].is_object(), "served body carries resources: {body}");
    assert_eq!(body["resources"], dumped["resources"], "served resources equal the dump's");

    let cached = http.get(&url).header("If-None-Match", &etag).send().await.expect("conditional GET");
    assert_eq!(cached.status(), 304);
    assert_eq!(cached.headers()["etag"].to_str().unwrap(), etag);
    assert!(cached.bytes().await.expect("body").is_empty(), "304 has an empty body");
}
