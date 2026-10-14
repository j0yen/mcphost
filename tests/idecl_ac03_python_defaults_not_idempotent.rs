//! PRD-mcphost-tool-idempotent-declaration AC3 -- Given a python tool
//! published with no flag, When listed, Then `idempotentHint: false` and
//! `destructiveHint: true`; Given `idempotent: true`, Then both flip.
//!
//! The rows are written straight to the tool store rather than through
//! `host.tool_publish`: a python publish builds its environment in the
//! sandbox (needs unprivileged user namespaces), which is not what this AC
//! measures -- the listing of a stored python spec is. The publish gate that
//! accepts `idempotent` is covered through the echo/http kinds in AC2.

use crate::common;
use common::{McpClient, TestServer, python_kind_registry, signup};
use serde_json::json;

const SOURCE: &str = "def main(args):\n    return {\"ok\": True}\n";

#[tokio::test]
async fn python_tool_defaults_to_not_idempotent_and_flag_flips_both_hints() {
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "IdeclAc03").await;
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns.clone())
        .await
        .unwrap()
        .expect("tenant");
    let client = McpClient::with_bearer(&server.base_url, &key);

    for (name, extra, idem) in [("plain_py", json!({}), false), ("upsert_py", json!({"idempotent": true}), true)] {
        let mut spec = json!({"source": SOURCE, "args_schema": {"type": "object"}});
        spec.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        server
            .state
            .db
            .upsert_tool(tenant.id, name.to_string(), "python".to_string(), spec, 5, vec![])
            .await
            .expect("store python tool");
        let tools = client.tools_list().await.expect("tools/list");
        let tool = tools["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == format!("{ns}.{name}"))
            .expect("listed");
        assert_eq!(tool["annotations"]["idempotentHint"], json!(idem), "{name}: {tool}");
        assert_eq!(tool["annotations"]["destructiveHint"], json!(!idem), "{name}: {tool}");
        assert_eq!(tool["annotations"]["readOnlyHint"], json!(false), "{name}: {tool}");
    }
}
