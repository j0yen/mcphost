//! AC2 -- Given a fake upstream answering 403, When the http tool is called,
//! Then what the client receives (the serialized error `data` plus the
//! message) names the CANONICAL `host.secret.set` in the registry `remedy`
//! (hotfix upstream-403-remedy-canonical-secret-set; discovery chain chaos
//! partial run 211800Z, family `upstream-remedy-missing:forbidden`: the
//! remedy used to name no tool, only the table's `hint` did), and never the
//! deprecated alias `host.secret_set`.

use crate::common;
use common::{McpClient, TestServer, http_kind_registry, signup};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn upstream_403_error_surface_names_host_secret_set_canonically() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(403).set_body_string("nope"))
        .mount(&upstream)
        .await;
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Upfam Ac2").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let spec = json!({
        "method": "GET",
        "url": format!("{}/v1/me", upstream.uri()),
        "args_schema": {"type": "object"},
    });
    client
        .tools_call("host.tool_publish", json!({"name": "me", "kind": "http", "spec": spec}))
        .await
        .expect("publish ok");
    let err = client
        .tools_call(&format!("{ns}.me"), json!({}))
        .await
        .expect_err("a 403 upstream must surface as a tool error");

    assert_eq!(err.error_code.as_deref(), Some("upstream_auth"), "{err:?}");
    assert_eq!(err.data["upstream_status"], 403);
    let remedy = err.data["remedy"].as_str().expect("data.remedy is a string");
    assert!(remedy.contains("host.secret.set"), "remedy must name the canonical tool: {remedy}");
    let received = format!("{}{}", serde_json::to_string(&err.data).unwrap(), err.message);
    assert!(received.contains("host.secret.set"), "{received}");
    assert!(!received.contains("host.secret_set"), "deprecated alias leaked into the error surface: {received}");
}
