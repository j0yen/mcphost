//! PRD-mcphost-python-kind-plain-env AC8 (P1) — Given a published tool with
//! a warm pooled process, When its env map is updated without source
//! changes, Then the next call observes the new values (the pooled process
//! was invalidated) and no republish of source occurred.

use crate::common;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry_with_warm_pool, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn updating_env_alone_is_observed_on_the_next_call() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry_with_warm_pool(
        &envs_dir.0,
        Duration::from_secs(60),
        2,
        16,
    ))
    .await;
    let (ns, key) = signup(&server.base_url, "AC8 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let source = "import os\ndef main(args):\n    return {\"mode\": os.environ.get('MODE')}\n";
    let spec_a = json!({
        "source": source,
        "args_schema": {"type": "object"},
        "env": {"MODE": "a"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "warm_env", "kind": "python", "spec": spec_a}),
        )
        .await
        .expect("publish ok");

    // First call: cold (seeds the warm pool). Second: likely a warm hit,
    // proving the pool serves this tool at all before the update below.
    for _ in 0..2 {
        let result = poll_until_ready(
            &client,
            &format!("{ns}.warm_env"),
            json!({}),
            Duration::from_secs(10),
        )
        .await
        .unwrap_or_else(|e| panic!("call must succeed: {} {}", e.code, e.message));
        assert_eq!(extract_structured(&result)["mode"], json!("a"));
    }

    // Same source, changed env only -- requirement 7's "no source change".
    let spec_b = json!({
        "source": source,
        "args_schema": {"type": "object"},
        "env": {"MODE": "b"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "warm_env", "kind": "python", "spec": spec_b}),
        )
        .await
        .expect("republish ok");

    let result = poll_until_ready(
        &client,
        &format!("{ns}.warm_env"),
        json!({}),
        Duration::from_secs(10),
    )
    .await
    .unwrap_or_else(|e| panic!("call must succeed: {} {}", e.code, e.message));
    assert_eq!(
        extract_structured(&result)["mode"],
        json!("b"),
        "the next call must observe the updated env value, not a stale pooled process's old one"
    );
}
