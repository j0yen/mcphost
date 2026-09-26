//! PRD-mcphost-run-result-overflow-to-state
//! AC5 (P0) — Given an http-kind async tool whose upstream returns 2 MiB,
//! When the run finishes, Then the same parts contract applies (no
//! `response_too_large`).

use crate::common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use serde_json::json;
use std::time::{Duration, Instant};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn oversized_async_http_result_overflows_into_parts_not_response_too_large() {
    let upstream = MockServer::start().await;
    let two_mib = vec![b'y'; 2 * 1024 * 1024];
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(two_mib))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "RunOverflow AC5 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "method": "GET",
        "url": format!("{}/v1/big", upstream.uri()),
        "args_schema": {"type": "object"},
        "response": "text",
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "bighttp", "kind": "http", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let enqueue = extract_structured(
        &client
            .tools_call(
                "host.tool_call",
                json!({"name": "bighttp", "args": {}, "async": true}),
            )
            .await
            .expect("enqueue ok"),
    );
    let run_id = enqueue["run_id"].as_str().expect("run_id").to_string();

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut done = None;
    while Instant::now() < deadline {
        let got = extract_structured(
            &client
                .tools_call("host.runs.wait", json!({"run_id": run_id, "timeout_s": 5}))
                .await
                .expect("wait ok"),
        );
        if got["status"] == json!("done") || got["status"] == json!("error") {
            done = Some(got);
            break;
        }
    }
    let done = done.expect("http job must finish within 10s");

    assert_eq!(done["status"], json!("done"), "run: {done}");
    assert_ne!(
        done["error_class"],
        json!("response_too_large"),
        "an async http call must never fail response_too_large: {done}"
    );
    let parts = done["result_ref"]["parts"].as_i64().expect("result_ref.parts must be present");
    assert!(parts >= 1, "run: {done}");

    let last = extract_structured(
        &client
            .tools_call("host.runs.part", json!({"run_id": run_id, "n": parts - 1}))
            .await
            .expect("last part ok"),
    );
    assert_eq!(last["n"], json!(parts - 1), "last: {last}");
    assert!(!last["data"].as_str().unwrap_or_default().is_empty(), "last: {last}");
}
