//! PRD-mcphost-python-kind-plain-env AC5 (P0) — Given a tool with secret
//! `API_KEY` and env `MODE=fast`, When `host.tool_test` runs, Then the
//! rendered environment shows `MODE=fast` verbatim, `API_KEY` redacted, and
//! each labeled as env or secret.

use crate::common;
use common::{TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn tool_test_shows_env_verbatim_and_secrets_redacted_labeled() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "AC5 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.secret_set",
            json!({"name": "API_KEY", "value": "sekret-9f2a"}),
        )
        .await
        .expect("secret_set ok");

    let spec = json!({
        "source": "def main(args):\n    return {}\n",
        "args_schema": {"type": "object"},
        "secrets": ["API_KEY"],
        "env": {"MODE": "fast"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "labeled_env", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let result = client
        .tools_call(
            "host.tool_test",
            json!({"name": "labeled_env", "args": {}}),
        )
        .await
        .expect("tool_test ok");
    let structured = extract_structured(&result);
    let environment = structured["environment"]
        .as_array()
        .expect("environment must be an array");

    let mode_entry = environment
        .iter()
        .find(|e| e["name"] == json!("MODE"))
        .expect("MODE entry must be present");
    assert_eq!(mode_entry["kind"], json!("env"));
    assert_eq!(mode_entry["value"], json!("fast"), "env value must be verbatim");

    let secret_entry = environment
        .iter()
        .find(|e| e["name"] == json!("API_KEY"))
        .expect("API_KEY entry must be present");
    assert_eq!(secret_entry["kind"], json!("secret"));
    assert_eq!(
        secret_entry["value"],
        json!("***"),
        "secret value must be redacted"
    );
}
