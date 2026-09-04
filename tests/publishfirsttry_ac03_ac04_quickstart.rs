//! AC3 -- Given `host.quickstart("python")` from a tenant, When called,
//! Then it returns the ordered sequence with the tenant's namespace filled
//! in and the current limits, and changes nothing.
//!
//! AC4 -- Given an unauthenticated `host.quickstart`, When called, Then it
//! returns the signup step first and no tenant data.

mod common;
use common::{TempDataDir, TestServer, all_kinds_registry, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn authenticated_quickstart_fills_namespace_and_limits_with_no_side_effects() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(all_kinds_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Quickstarter").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let result = client
        .tools_call("host.quickstart", json!({"kind": "python"}))
        .await
        .expect("quickstart");
    let structured = extract_structured(&result);

    assert_eq!(structured["authenticated"], json!(true));
    assert_eq!(structured["namespace"], json!(ns));
    assert_eq!(structured["kind"], json!("python"));

    let steps = structured["steps"].as_array().expect("steps array");
    assert!(
        !steps.is_empty(),
        "quickstart must return at least one step"
    );
    // Every step names a real call, and the calls collectively cover
    // publish, test, and the real invocation.
    let calls: Vec<&str> = steps.iter().filter_map(|s| s["call"].as_str()).collect();
    assert!(calls.contains(&"host.tool_publish"), "steps: {calls:?}");
    assert!(calls.contains(&"host.tool_test"), "steps: {calls:?}");
    assert!(
        calls
            .iter()
            .any(|c| *c == format!("{ns}.my_tool") || *c == "host.tool_call"),
        "steps must include the real, namespaced call: {calls:?}"
    );
    // The publish step's spec is real and kind-appropriate, not a stub.
    let publish_step = steps
        .iter()
        .find(|s| s["call"] == json!("host.tool_publish"))
        .expect("publish step present");
    assert!(
        publish_step["arguments"]["spec"]["source"].is_string(),
        "python's example spec must carry a 'source' field: {publish_step}"
    );

    let limits = &structured["limits"];
    assert_eq!(
        limits["max_spec_bytes"],
        json!(mcphost::state::MAX_SPEC_BYTES)
    );
    assert_eq!(
        limits["max_tools_per_tenant"],
        json!(mcphost::state::MAX_TOOLS_PER_TENANT)
    );
    assert!(limits["name_pattern"].as_str().is_some());

    // No side effects: quickstart must not have published anything.
    let tools = client
        .tools_call("host.tool_list", json!({}))
        .await
        .expect("tool_list");
    let tools = extract_structured(&tools);
    assert_eq!(
        tools["tools"].as_array().map(|v| v.len()),
        Some(0),
        "host.quickstart must publish nothing: {tools}"
    );
}

#[tokio::test]
async fn quickstart_names_the_registered_kind_for_an_unregistered_request() {
    let server = TestServer::start().await; // echo only
    let (_ns, key) = signup(&server.base_url, "Quickstarter").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call("host.quickstart", json!({"kind": "not_a_kind"}))
        .await
        .expect_err("unregistered kind must fail");
    assert_eq!(err.error_code.as_deref(), Some("unknown_kind"));
}

#[tokio::test]
async fn unauthenticated_quickstart_returns_signup_step_and_no_tenant_data() {
    let server = TestServer::start().await;
    let client = common::McpClient::new(&server.base_url);

    let result = client
        .tools_call("host.quickstart", json!({"kind": "python"}))
        .await
        .expect("quickstart is callable unauthenticated");
    let structured = extract_structured(&result);

    assert_eq!(structured["authenticated"], json!(false));
    assert!(
        structured.get("namespace").is_none(),
        "no tenant data: {structured}"
    );
    assert!(
        structured.get("limits").is_none(),
        "no tenant data: {structured}"
    );

    let steps = structured["steps"].as_array().expect("steps array");
    assert_eq!(
        steps.len(),
        1,
        "unauthenticated quickstart returns signup first: {structured}"
    );
    assert_eq!(steps[0]["call"], json!("signup"));
}

/// Same as above, but with no `kind` argument at all -- the unauthenticated
/// branch must not require one (there is nothing tenant-specific to fill
/// in yet).
#[tokio::test]
async fn unauthenticated_quickstart_works_with_no_kind_argument() {
    let server = TestServer::start().await;
    let client = common::McpClient::new(&server.base_url);

    let result = client
        .tools_call("host.quickstart", json!({}))
        .await
        .expect("quickstart with no kind must still work unauthenticated");
    let structured = extract_structured(&result);
    assert_eq!(structured["authenticated"], json!(false));
}
