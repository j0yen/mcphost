//! AC19 — Given the registry feature flag is on and the domain namespace
//! is verified, When `host.registry_publish()` runs, Then
//! `/.well-known/mcp/<namespace>/server.json` serves a valid document with
//! a `streamable-http` remote and the registry API accepts it (mocked
//! here with `wiremock`); Given the flag is off, or the namespace is not
//! verified, Then `host.registry_publish` refuses with a distinct error.
//!
//! What this deliberately does NOT test: the domain-namespace
//! VERIFICATION METHOD (DNS vs HTTP) -- the PRD's Open Questions leaves
//! that to Joe. "Verified" here is only ever the boolean
//! `admin.tenant_verify_namespace` sets.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, signup};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn flag_off_refuses_registry_publish() {
    // No registry configured at all -- `TestServer::start()` never sets
    // `AppState::registry`.
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "No Registry Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call("host.registry_publish", json!({}))
        .await
        .expect_err("registry_publish must refuse when the feature flag is off");
    assert_eq!(err.error_code.as_deref(), Some("registry_disabled"));
}

#[tokio::test]
async fn unverified_namespace_refuses_registry_publish() {
    let mock = MockServer::start().await;
    let server = TestServer::start_with_registry(mock.uri()).await;
    let (_ns, key) = signup(&server.base_url, "Unverified Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call("host.registry_publish", json!({}))
        .await
        .expect_err("registry_publish must refuse an unverified tenant even with the flag on");
    assert_eq!(err.error_code.as_deref(), Some("namespace_unverified"));

    // The negative path must not have hit the registry API at all.
    assert_eq!(
        mock.received_requests().await.map(|r| r.len()).unwrap_or(0),
        0,
        "an unverified publish must never reach the registry API"
    );
}

#[tokio::test]
async fn verified_tenant_publishes_and_well_known_serves_the_document() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v0/publish"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "accepted"})))
        .mount(&mock)
        .await;

    let server = TestServer::start_with_registry(mock.uri()).await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let (tenant_ns, key) = signup(&server.base_url, "Verified Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let domain_namespace = "io.github.j0yen.verified-tenant";
    admin
        .tools_call(
            "admin.tenant_verify_namespace",
            json!({"tenant": tenant_ns, "domain_namespace": domain_namespace}),
        )
        .await
        .expect("admin.tenant_verify_namespace");

    let result = client
        .tools_call("host.registry_publish", json!({}))
        .await
        .expect("registry_publish must succeed for a verified tenant with the flag on");
    let published = common::extract_structured(&result);
    assert_eq!(published["domain_namespace"], domain_namespace);
    assert_eq!(published["registry_status"], 200);

    // The registry API really was called, with a server.json-shaped body.
    let requests = mock.received_requests().await.expect("recording enabled");
    assert_eq!(requests.len(), 1, "registry_publish must POST exactly once");
    let body: serde_json::Value = requests[0].body_json().expect("registry API request body");
    assert_eq!(body["name"], domain_namespace);
    let remotes = body["remotes"].as_array().expect("remotes array");
    assert_eq!(remotes.len(), 1);
    assert_eq!(remotes[0]["type"], "streamable-http");
    assert!(
        remotes[0]["url"].as_str().unwrap().ends_with("/mcp"),
        "remote url must point at this host's /mcp endpoint: {remotes:?}"
    );

    // `/.well-known/mcp/<namespace>/server.json` now serves that same
    // document, unauthenticated.
    let http = reqwest::Client::new();
    let resp = http
        .get(format!(
            "{}/.well-known/mcp/{}/server.json",
            server.base_url, tenant_ns
        ))
        .send()
        .await
        .expect("GET well-known server.json");
    assert_eq!(resp.status(), 200);
    let doc: serde_json::Value = resp.json().await.expect("parse server.json");
    assert_eq!(doc["name"], domain_namespace);
    let doc_remotes = doc["remotes"].as_array().expect("remotes array");
    assert_eq!(doc_remotes[0]["type"], "streamable-http");
    assert!(doc_remotes[0]["url"].as_str().unwrap().ends_with("/mcp"));
}

#[tokio::test]
async fn well_known_404s_for_a_namespace_that_never_published() {
    let server = TestServer::start().await;
    let http = reqwest::Client::new();
    let resp = http
        .get(format!(
            "{}/.well-known/mcp/t_deadbeef/server.json",
            server.base_url
        ))
        .send()
        .await
        .expect("GET well-known server.json");
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn registry_api_rejection_is_a_distinct_error() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v0/publish"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({"error": "bad namespace"})))
        .mount(&mock)
        .await;

    let server = TestServer::start_with_registry(mock.uri()).await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let (tenant_ns, key) = signup(&server.base_url, "Rejected Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    admin
        .tools_call(
            "admin.tenant_verify_namespace",
            json!({"tenant": tenant_ns, "domain_namespace": "io.github.j0yen.rejected-tenant"}),
        )
        .await
        .expect("admin.tenant_verify_namespace");

    let err = client
        .tools_call("host.registry_publish", json!({}))
        .await
        .expect_err("a non-2xx from the registry API must propagate as an error");
    assert_eq!(err.error_code.as_deref(), Some("registry_rejected"));
}
