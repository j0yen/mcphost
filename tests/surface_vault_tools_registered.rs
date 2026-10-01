//! Regression guard for the mcphost.dev promise-ledger finding (PR #97):
//! `handler.rs`'s `call_tool` dispatches `host.vault.provider_set`,
//! `host.vault.providers`, `host.vault.connect_link`, `host.vault.status`,
//! `host.vault.disconnect`, and `host.vault.provider_remove` (see the
//! `match name` arms in `call_tool`), but `host_tools()` never pushed a
//! `Tool::new` entry for any of them -- callable, yet absent from
//! `tools/list` (and therefore from `www/llms.txt`'s generated Tools
//! section and `contracts/host-tools.v1.json`). This test fails on current
//! main: `tools/list` for a freshly signed-up tenant contains none of the
//! six names.
//!
//! Scope: the tenant-visible `host.*` surface built by `handler.rs`'s
//! `host_tools` (PRD-mcphost-upstream-token-vault / -upstream-token-vault-status) --
//! not `admin.vault.stats`, which is already registered in `admin_tools()`.

use crate::common;
use common::{TempDataDir, TestServer, all_kinds_registry, signup};
use serde_json::Value;

const EXPECTED_VAULT_TOOLS: &[(&str, &[&str])] = &[
    (
        "host.vault.provider_set",
        &["name", "client_id", "client_secret", "scopes"],
    ),
    ("host.vault.providers", &[]),
    ("host.vault.connect_link", &["provider", "end_user"]),
    ("host.vault.status", &["end_user"]),
    ("host.vault.disconnect", &["provider", "end_user"]),
    ("host.vault.provider_remove", &["name"]),
];

#[tokio::test]
async fn tools_list_registers_every_host_vault_tool_with_a_schema() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(all_kinds_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Surface Vault Tools Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let result = client.tools_list().await.expect("tools/list");
    let tools = result["tools"].as_array().expect("tools array");

    for (name, required) in EXPECTED_VAULT_TOOLS {
        let tool = tools
            .iter()
            .find(|t| t["name"] == *name)
            .unwrap_or_else(|| {
                panic!(
                    "{name} must be registered in tools/list, got: {:?}",
                    tools
                        .iter()
                        .filter_map(|t| t["name"].as_str())
                        .collect::<Vec<_>>()
                )
            });

        let description = tool["description"].as_str().unwrap_or("");
        assert!(
            description.len() >= 20,
            "{name} description must be at least 20 chars, got {} ({description:?})",
            description.len()
        );

        let schema = &tool["inputSchema"];
        let properties = schema["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("{name} inputSchema must have properties"));
        let schema_required: Vec<&str> = schema["required"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();

        for field in *required {
            assert!(
                schema_required.contains(field),
                "{name} must declare '{field}' as required, got required={schema_required:?}"
            );
            let desc = properties
                .get(*field)
                .and_then(|p| p.get("description"))
                .and_then(Value::as_str)
                .unwrap_or("");
            assert!(
                !desc.is_empty(),
                "{name}.{field} (required) must have a non-empty description"
            );
        }

        // Every host.* tool carries the optional tenant_key property.
        assert!(
            properties.contains_key("tenant_key"),
            "{name} must carry the tenant_key property every host.* tool gets"
        );
    }
}
