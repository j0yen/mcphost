//! PRD-mcphost-tool-infer AC15 (P0) — Given a spec published without
//! `args_schema`, When `host.tool_test` is called for that tool, Then the
//! response shows the inferred schema.
//!
//! Both kinds' `call()` already had a `ctx.test_mode`-only debug-info path
//! (`http` echoes the rendered request); this PRD extends that same
//! pattern -- entirely inside `kinds::python`/`kinds::http`, no
//! `handler.rs` change -- so a `host.tool_test` response carries the
//! `schema` actually used (inferred or authored) alongside the tool's
//! result. This test asserts the schema is literally present and correct
//! in the response, not merely that a missing-key call gets rejected.

mod common;
use common::{TestServer, extract_structured, http_kind_registry, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn tool_test_response_carries_the_inferred_python_schema() {
    // Requirement 8/9: this test builds and runs a real python-kind tool
    // via the sandbox, which needs unprivileged user namespaces. Not
    // guaranteed on GitHub's hosted runners, so skip cleanly in CI (and
    // fail loudly, not skip, anywhere else -- see
    // require_user_namespaces_or_ci_skip's doc comment) rather than fail
    // with "the tool's environment failed to build" -- same pattern as the
    // sandbox-dependent unit tests in src/kinds/python.rs and
    // src/sandbox.rs, and as tests/ac17_kind_conformance.rs.
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("skipped: no user namespaces (CI)");
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC15a Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "def main(args):\n    return {\"city\": args[\"city\"]}\n",
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "needs_city", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    // Let the (empty-requirements) env finish building before exercising
    // tool_test, the same way every other python AC test does.
    poll_until_ready(
        &client,
        &format!("{ns}.needs_city"),
        json!({"city": "Reno"}),
        Duration::from_secs(60),
    )
    .await
    .expect("env must become ready");

    let result = client
        .tools_call(
            "host.tool_test",
            json!({"name": "needs_city", "args": {"city": "Reno"}}),
        )
        .await
        .expect("tool_test ok");
    let structured = extract_structured(&result);

    // The result is still reachable ...
    assert_eq!(structured["result"]["city"], "Reno");
    // ... and the inferred schema is genuinely shown, not just enforced.
    assert_eq!(structured["schema"]["required"], json!(["city"]));
    assert!(structured["schema"]["properties"]["city"].is_object());
}

#[tokio::test]
async fn tool_test_response_carries_the_inferred_http_schema() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "AC15b Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    // No upstream call needs to succeed for this assertion -- the schema is
    // attached to the test-mode response regardless of upstream status, so
    // point at an address nothing is listening on and only check the
    // schema is present on whatever error/response comes back is not
    // viable (`call` returns Err on a failed request, which carries no
    // response body). Use a real, reachable stub instead.
    let upstream = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(&upstream)
        .await;

    let spec = json!({
        "method": "GET",
        "url": format!("{}/v1/{{{{ city }}}}", upstream.uri()),
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "wrapper", "kind": "http", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let result = client
        .tools_call(
            "host.tool_test",
            json!({"name": "wrapper", "args": {"city": "reno"}}),
        )
        .await
        .expect("tool_test ok");
    let structured = extract_structured(&result);

    assert_eq!(structured["response"]["status"], 200);
    assert_eq!(structured["schema"]["required"], json!(["city"]));
    assert!(structured["schema"]["properties"]["city"].is_object());
}
