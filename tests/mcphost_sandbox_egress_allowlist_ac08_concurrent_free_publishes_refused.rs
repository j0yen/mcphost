//! PRD-mcphost-sandbox-egress-allowlist
//! AC8 (P0) — Given 20 concurrent Free publishes with `network: "public"`,
//! When they complete, Then all 20 are refused and tool count is
//! unchanged.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

const N: usize = 20;

#[tokio::test]
async fn twenty_concurrent_free_publishes_are_all_refused() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "AC8 Tenant").await;

    let mut handles = Vec::with_capacity(N);
    for i in 0..N {
        let base_url = server.base_url.clone();
        let key = key.clone();
        handles.push(tokio::spawn(async move {
            let client = McpClient::with_bearer(&base_url, &key);
            client
                .tools_call(
                    "host.tool_publish",
                    json!({
                        "name": format!("downloader_{i}"),
                        "kind": "python",
                        "spec": {
                            "source": "def main(args):\n    return {\"ok\": True}\n",
                            "args_schema": {"type": "object"},
                            "network": "public",
                        },
                    }),
                )
                .await
        }));
    }

    let mut refused = 0;
    for handle in handles {
        let result = handle.await.expect("task must not panic");
        let err = result.expect_err("every concurrent free publish must be refused");
        assert_eq!(err.error_code.as_deref(), Some("plan_required"));
        refused += 1;
    }
    assert_eq!(refused, N);

    let client = McpClient::with_bearer(&server.base_url, &key);
    let tools = extract_structured(
        &client.tools_call("host.tool_list", json!({})).await.expect("tool_list"),
    );
    assert_eq!(
        tools["tools"].as_array().expect("tools array").len(),
        0,
        "tool count must be unchanged: {tools:?}"
    );
}
