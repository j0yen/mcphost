//! AC7 — Given the same 401 upstream, When `host.tool_test` runs the tool,
//! Then the response carries the identical code, `data.upstream_status`, and
//! hint as a real call.

use crate::common;
use common::{McpClient, TestServer, http_kind_registry, signup};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn tool_test_and_real_call_agree_on_code_status_and_hint() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401).set_body_string("no key"))
        .mount(&upstream)
        .await;
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Upfam Parity").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let spec = json!({
        "method": "GET",
        "url": format!("{}/v1/me", upstream.uri()),
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "me", "kind": "http", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let real = client
        .tools_call(&format!("{ns}.me"), json!({}))
        .await
        .expect_err("real call surfaces the 401");
    let tested = client
        .tools_call("host.tool_test", json!({"name": "me", "args": {}}))
        .await
        .expect_err("host.tool_test surfaces the same 401");

    assert_eq!(real.error_code.as_deref(), Some("upstream_auth"));
    assert_eq!(tested.error_code, real.error_code);
    assert_eq!(tested.data["upstream_status"], real.data["upstream_status"]);
    assert_eq!(tested.data["upstream_status"], 401);
    assert_eq!(tested.data["hint"], real.data["hint"]);
    assert!(tested.data["hint"].as_str().is_some_and(|h| h.contains("host.secret.set")));
}
