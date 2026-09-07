//! PRD-mcphost-rest-bridge AC3 — Given a spec with `upstream` mapping
//! params to query and path and a secret-referenced auth header, When
//! published and called against a local mock upstream, Then no user
//! request code was involved and `result.payload` carries status and
//! parsed body with the mapped request observed by the mock.

mod common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn upstream_spec_maps_path_query_and_secret_auth_header_with_no_hand_written_request_code() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/orders/order-42"))
        .and(header("Authorization", "Bearer topsecretvalue"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "order_id": "order-42",
            "status": "shipped",
        })))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Bridge Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.secret_set",
            json!({"name": "api_key", "value": "topsecretvalue"}),
        )
        .await
        .expect("secret_set ok");

    // A declarative bridge spec: no `{{ }}` template written by hand
    // anywhere -- the param -> query/path mapping and the auth header are
    // entirely spec data.
    let spec = json!({
        "upstream": {
            "url": format!("{}/orders/{{order_id}}", upstream.uri()),
            "method": "GET",
            "params": {
                "order_id": {"in": "path"},
                "note": {"in": "query"},
            },
            "auth": {"header": "Authorization", "secret": "api_key", "prefix": "Bearer "},
        },
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "orders", "kind": "http", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let result = client
        .tools_call(
            &format!("{ns}.orders"),
            json!({"order_id": "order-42", "note": "priority"}),
        )
        .await
        .expect("call ok");
    let structured = extract_structured(&result);

    assert_eq!(structured["status"], 200);
    assert_eq!(structured["payload"]["order_id"], "order-42");
    assert_eq!(structured["payload"]["status"], "shipped");

    let requests = upstream
        .received_requests()
        .await
        .expect("mock server tracks requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].url.path(), "/orders/order-42");
    assert_eq!(
        requests[0]
            .url
            .query_pairs()
            .find(|(k, _)| k == "note")
            .map(|(_, v)| v.into_owned()),
        Some("priority".to_string())
    );
}

#[tokio::test]
async fn upstream_spec_and_direct_method_url_together_are_rejected() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Conflicting Spec Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "method": "GET",
        "url": "https://api.example.com/x",
        "upstream": {"url": "https://api.example.com/y", "method": "GET", "params": {}},
    });
    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "conflict", "kind": "http", "spec": spec}),
        )
        .await
        .expect_err("declaring both upstream and method/url must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("invalid_spec"));
}

#[tokio::test]
async fn a_path_param_missing_from_the_upstream_url_is_rejected_at_publish() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Bad Path Param Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "upstream": {
            "url": "https://api.example.com/orders",
            "method": "GET",
            "params": {"order_id": {"in": "path"}},
        },
    });
    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "badpath", "kind": "http", "spec": spec}),
        )
        .await
        .expect_err("a path param not referenced in upstream.url must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("invalid_spec"));
}
