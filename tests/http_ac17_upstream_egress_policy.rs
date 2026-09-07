//! PRD-mcphost-rest-bridge AC5 — Given the http kind's current egress
//! policy tests, When bridge calls run, Then the same policy verdicts
//! apply to `upstream` requests. A compiled `upstream` spec becomes an
//! ordinary [`mcphost::kinds::http::HttpKind`] spec (see `compile_upstream`
//! in `src/kinds/http.rs`), so it runs through the identical SSRF
//! (`http_ac02`) and DNS-rebinding (`http_ac09`) checks -- these tests
//! prove that's true for the `upstream` shape specifically, not just the
//! hand-templated one.

mod common;
use common::{FixedLookup, McpClient, TestServer, http_kind_registry_strict, signup};
use serde_json::json;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};

#[tokio::test]
async fn an_upstream_spec_pointing_at_a_private_literal_is_rejected_at_publish() {
    let server =
        TestServer::start_with_kinds(http_kind_registry_strict(FixedLookup(HashMap::new()))).await;
    let (_ns, key) = signup(&server.base_url, "Bridge SSRF Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "upstream": {
            "url": "https://10.0.0.5/orders/{order_id}",
            "method": "GET",
            "params": {"order_id": {"in": "path"}},
        },
    });
    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "bad_bridge", "kind": "http", "spec": spec}),
        )
        .await
        .expect_err("an upstream spec targeting a private literal must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("host_not_allowed"));
}

#[tokio::test]
async fn an_upstream_spec_hostname_rebinding_to_a_private_address_is_refused_at_call_time() {
    let mut answers = HashMap::new();
    answers.insert(
        "bridge-rebind.example".to_string(),
        vec![IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3))],
    );
    let server =
        TestServer::start_with_kinds(http_kind_registry_strict(FixedLookup(answers))).await;
    let (ns, key) = signup(&server.base_url, "Bridge Rebind Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // A hostname, not a literal IP -- publishes fine (no DNS answer yet to
    // judge), exactly like the hand-templated case in `http_ac09`.
    let spec = json!({
        "upstream": {
            "url": "https://bridge-rebind.example/orders/{order_id}",
            "method": "GET",
            "params": {"order_id": {"in": "path"}},
        },
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "rebind_bridge", "kind": "http", "spec": spec}),
        )
        .await
        .expect("a hostname that isn't obviously private must publish fine");

    let err = client
        .tools_call(&format!("{ns}.rebind_bridge"), json!({"order_id": "42"}))
        .await
        .expect_err("a hostname resolving to a private address must be refused at call time");
    assert_eq!(err.error_code.as_deref(), Some("host_not_allowed"));
}
