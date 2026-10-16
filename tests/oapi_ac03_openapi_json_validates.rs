//! PRD-mcphost-openapi-for-rest-facade
//! AC3 (P0) -- Given `GET /openapi.json`, When validated against
//! `schemas/openapi-3.1.json`, Then it passes, `info.x-contract-version`
//! equals `host.whoami.contract_version`, and the ETag equals sha256 of the
//! body.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::http::routes::ROUTES;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[tokio::test]
async fn oapi_ac03() {
    let server = TestServer::start().await;
    let resp = reqwest::get(format!("{}/openapi.json", server.base_url))
        .await
        .expect("GET /openapi.json");
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers()["content-type"], "application/json");
    assert_eq!(resp.headers()["cache-control"], "max-age=300");
    let etag = resp.headers()["etag"].to_str().expect("etag ascii").to_string();
    let bytes = resp.bytes().await.expect("body");
    let doc: Value = serde_json::from_slice(&bytes).expect("body is JSON");

    // Validates against the vendored OpenAPI 3.1 schema.
    let schema: Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/schemas/openapi-3.1.json")).expect("schemas/openapi-3.1.json"),
    )
    .expect("vendored schema is JSON");
    let validator = jsonschema::validator_for(&schema).expect("vendored OpenAPI schema compiles");
    let errors: Vec<String> = validator.iter_errors(&doc).map(|e| format!("{} at {}", e, e.instance_path)).collect();
    assert!(errors.is_empty(), "document violates OpenAPI 3.1: {errors:#?}");
    // The validator is not vacuous: a document missing `info` is refused.
    assert!(!validator.is_valid(&json!({"openapi": "3.1.0", "paths": {}})));
    assert!(!validator.is_valid(&json!({"openapi": "3.0.3", "info": {"title": "t", "version": "1"}, "paths": {}})));

    // info.x-contract-version == host.whoami.contract_version.
    let (_ns, key) = signup(&server.base_url, "OAPI AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let whoami = extract_structured(&client.tools_call("host.whoami", json!({})).await.expect("host.whoami"));
    let contract_version = whoami["contract_version"].as_str().expect("whoami.contract_version");
    assert_eq!(doc["info"]["x-contract-version"], json!(contract_version));
    assert_eq!(doc["info"]["version"], json!(env!("CARGO_PKG_VERSION")));

    // ETag == sha256 of the body.
    let digest: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(etag, format!("\"{digest}\""));

    // A matching If-None-Match is a 304 with the same ETag.
    let cached = reqwest::Client::new()
        .get(format!("{}/openapi.json", server.base_url))
        .header("if-none-match", &etag)
        .send()
        .await
        .expect("conditional GET");
    assert_eq!(cached.status(), 304);

    // Every ROUTES entry is documented, method by method.
    for route in ROUTES {
        let op = &doc["paths"][route.path][route.method.to_ascii_lowercase()];
        assert!(op.is_object(), "{} {} missing from the document", route.method, route.path);
    }
    assert_eq!(doc["servers"], json!([{"url": "https://mcphost.dev"}]));

    // /docs loads Redoc from cdnjs and no other external script.
    let docs = reqwest::get(format!("{}/docs", server.base_url)).await.expect("GET /docs");
    assert_eq!(docs.status(), 200);
    let html = docs.text().await.expect("docs body");
    let scripts: Vec<&str> = html.match_indices("<script src=\"").map(|(i, m)| &html[i + m.len()..]).collect();
    assert_eq!(scripts.len(), 1, "exactly one script: {html}");
    assert!(
        scripts[0].starts_with("https://cdnjs.cloudflare.com/ajax/libs/redoc/2.1.5/bundles/redoc.standalone.js\""),
        "{}",
        scripts[0]
    );
}
