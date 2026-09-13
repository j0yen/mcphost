//! PRD-mcphost-python-kind-plain-env AC7 (P0) — Given the descriptor and
//! llms.txt after build, When grepped, Then `env` is documented with its
//! bounds and the env-versus-secret distinction, and `host.tool_get` (here:
//! `host.tool_list`, the descriptor surface that already returns a tool's
//! parsed spec) returns the env map for a tool that has one.

use crate::common;
use common::{TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

const PYTHON_KIND_DOC: &str = include_str!("../docs/kinds/python.md");
const LLMS_TXT: &str = include_str!("../www/llms.txt");
const README: &str = include_str!("../README.md");

#[test]
fn python_kind_doc_documents_env_bounds_and_the_secret_distinction() {
    assert!(
        PYTHON_KIND_DOC.contains("\"env\""),
        "docs/kinds/python.md must show an env example"
    );
    assert!(
        PYTHON_KIND_DOC.contains("16 entries") && PYTHON_KIND_DOC.contains("4 KiB"),
        "docs/kinds/python.md must state the env bounds"
    );
    assert!(
        PYTHON_KIND_DOC.to_lowercase().contains("distinct from") || PYTHON_KIND_DOC.contains("redacted"),
        "docs/kinds/python.md must describe the env-vs-secret distinction"
    );
}

#[test]
fn llms_txt_and_readme_mention_env() {
    assert!(
        LLMS_TXT.contains("`env`"),
        "www/llms.txt must mention the env map"
    );
    assert!(README.contains("`env`"), "README.md must mention the env map");
}

#[tokio::test]
async fn tool_list_returns_the_env_map_for_a_tool_that_has_one() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC7 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "def main(args):\n    return {}\n",
        "args_schema": {"type": "object"},
        "env": {"MODE": "fast"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "described", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let result = client
        .tools_call("host.tool_list", json!({}))
        .await
        .expect("tool_list ok");
    let structured = extract_structured(&result);
    let tools = structured["tools"].as_array().expect("tools array");
    let tool = tools
        .iter()
        .find(|t| t["name"] == json!(format!("{ns}.described")))
        .expect("published tool must be listed");
    assert_eq!(tool["env"], json!({"MODE": "fast"}));
}
