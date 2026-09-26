//! PRD-mcphost-first-publish-real-kind
//! AC1 (P0) -- Given a fresh free-plan tenant, When `host.quickstart` is
//! called, Then `starter_tool.kind == "python"` and executing `publish_call`
//! then `test_call` verbatim returns `{reversed: "olleh", words: 1}` for
//! input "hello".

use crate::common;
use common::{TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn quickstart_starter_tool_publishes_and_runs_for_real() {
    // This test publishes and runs a real sandboxed python tool, which
    // needs unprivileged user namespaces -- not guaranteed on GitHub's
    // hosted runners. Same skip convention every other sandbox-dependent
    // test in this crate uses.
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Starter Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let result = client
        .tools_call("host.quickstart", json!({}))
        .await
        .expect("quickstart is callable with no arguments");
    let structured = extract_structured(&result);

    let starter = &structured["starter_tool"];
    assert_eq!(
        starter["kind"],
        json!("python"),
        "the documented first publish must be a real python tool: {structured}"
    );
    assert!(
        starter["name"].as_str().is_some(),
        "starter_tool must name itself: {starter}"
    );

    // Execute publish_call, then test_call, verbatim -- exactly the two
    // calls host.quickstart handed back, with no extra polling or coaxing.
    let publish_call = &starter["publish_call"];
    client
        .tools_call(
            publish_call["call"].as_str().expect("publish_call.call"),
            publish_call["arguments"].clone(),
        )
        .await
        .expect("starter_tool.publish_call must publish on the free plan");

    let test_call = &starter["test_call"];
    let test_result = client
        .tools_call(
            test_call["call"].as_str().expect("test_call.call"),
            test_call["arguments"].clone(),
        )
        .await
        .expect("starter_tool.test_call must succeed");
    let test_structured = extract_structured(&test_result);

    assert_eq!(
        test_structured["result"]["reversed"],
        json!("olleh"),
        "reversing 'hello' must return 'olleh': {test_structured}"
    );
    assert_eq!(
        test_structured["result"]["words"],
        json!(1),
        "'hello' is one word: {test_structured}"
    );
}
