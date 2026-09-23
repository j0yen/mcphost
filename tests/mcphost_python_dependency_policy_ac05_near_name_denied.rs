//! PRD-mcphost-python-dependency-policy
//! AC5 (P0) — Given `reqeusts` in requirements, When published, Then it
//! fails with `dependency_policy: denied (near-name of requests)`.

use crate::common;
use common::{TestServer, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn typosquat_near_name_denies_publish_before_resolution() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "AC5 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "def main(args):\n    return {\"ok\": True}\n",
        "requirements": ["reqeusts"],
        "args_schema": {"type": "object"},
    });
    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "typo", "kind": "python", "spec": spec}),
        )
        .await
        .expect_err("a near-name typosquat must never reach resolution, let alone publish");

    assert_eq!(err.error_code.as_deref(), Some("dependency_policy"), "{err:?}");
    assert_eq!(
        err.message, "dependency_policy: denied (near-name of requests)",
        "{err:?}"
    );
}
