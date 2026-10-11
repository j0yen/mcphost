//! PRD-mcphost-uptime-probe-recipe-green
//! AC5 -- Given `host.tool_publish(name="probe", kind="python", spec={network:
//! "public"})` on free, When refused, Then the error keeps its text and carries
//! `data.alternative == "host.uptime.create"`.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn refused_network_public_keeps_its_text_and_names_host_uptime_create() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "uprg ac5").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "def main(args):\n    return {\"ok\": True}\n",
        "args_schema": {"type": "object"},
        "network": "public",
    });
    let err = client
        .tools_call("host.tool_publish", json!({"name": "probe", "kind": "python", "spec": spec}))
        .await
        .expect_err("free + network public is refused");

    assert_eq!(err.error_code.as_deref(), Some("plan_required"), "{err:?}");
    assert!(err.message.contains("requires the pro plan"), "the error keeps its text: {err:?}");
    assert_eq!(err.data["plan"], json!("pro"), "{err:?}");
    assert_eq!(err.data["alternative"], json!("host.uptime.create"), "{err:?}");
    assert!(err.data["docs"].is_string(), "{err:?}");
}
