//! PRD-mcphost-chain-host-steps AC5 (P0) — Given `host.quickstart
//! kind=chain`, When its `steps[0].arguments.spec` is passed to
//! `host.spec_test` unchanged on a tenant that published the quickstart's
//! python starter, Then spec_test passes with every step `resolved`.

use crate::common;
use common::{TestServer, chain_and_python_kind_registry, extract_structured, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn quickstart_chain_example_resolves_once_the_python_starter_is_published() {
    // The python starter's own publish goes through the real sandbox (its
    // source is parsed/checked there, never in the host process) -- same
    // skip as every other python-kind test in this suite on a host with no
    // unprivileged user namespaces.
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(chain_and_python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Chain Quickstart Example Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    // Publish "the quickstart's python starter" by literally replaying
    // host.quickstart(kind="python")'s own documented publish_call, so this
    // test can never drift from whatever that starter actually is.
    let python_quickstart = extract_structured(
        &client
            .tools_call("host.quickstart", json!({"kind": "python"}))
            .await
            .expect("host.quickstart kind=python must succeed"),
    );
    let publish_call = &python_quickstart["starter_tool"]["publish_call"];
    client
        .tools_call(
            publish_call["call"].as_str().expect("publish_call.call"),
            publish_call["arguments"].clone(),
        )
        .await
        .expect("publishing the quickstart python starter must succeed");

    let chain_quickstart = extract_structured(
        &client
            .tools_call("host.quickstart", json!({"kind": "chain"}))
            .await
            .expect("host.quickstart kind=chain must succeed"),
    );
    let publish_step = chain_quickstart["steps"][0].clone();
    assert_eq!(publish_step["call"], json!("host.tool_publish"));
    let spec = publish_step["arguments"]["spec"].clone();

    let result = extract_structured(
        &client
            .tools_call(
                "host.spec_test",
                json!({"kind": "chain", "spec": spec, "invocations": [{"text": "hello"}]}),
            )
            .await
            .expect("host.spec_test on the quickstart chain example must succeed"),
    );
    let invocation = &result["invocations"][0];
    assert_eq!(
        invocation["ok"], json!(true),
        "the quickstart chain example must spec_test ok as printed: {result}"
    );
    let steps = invocation["output"]["steps"]
        .as_array()
        .expect("dry-run steps report");
    assert!(!steps.is_empty());
    for step in steps {
        let resolved = step["resolved"].as_str().unwrap_or_default();
        assert!(
            resolved == "tenant" || resolved == "host",
            "every step must be resolved (tenant or host): {step}"
        );
    }
}
