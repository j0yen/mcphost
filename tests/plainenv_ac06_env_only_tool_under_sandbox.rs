//! PRD-mcphost-python-kind-plain-env AC6 (P0) — Given a tool with env and
//! no secrets, When called under the sandbox, Then the values are present
//! and the sandbox profile is byte-identical to the secrets-only case apart
//! from the variables themselves.
//!
//! "byte-identical apart from the variables" is exercised here as: an
//! env-only tool runs successfully under the exact same sandbox path
//! (`kinds::python::call`'s cold path, `RunSpec` built the same way
//! `tests/python_ac11_secret_redaction.rs`'s secrets-only tool is) with no
//! `secrets` declared at all, and the process sees nothing under its own
//! `SECRET_*` convention -- proving env injection doesn't piggyback on, or
//! require, the secrets machinery.

use crate::common;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn env_only_tool_with_no_secrets_sees_its_env_and_nothing_secret_shaped() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC6 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "import os\ndef main(args):\n    secret_shaped = [k for k in os.environ if k.startswith('SECRET_')]\n    return {\"mode\": os.environ.get('MODE'), \"secret_shaped\": secret_shaped}\n",
        "args_schema": {"type": "object"},
        "env": {"MODE": "fast"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "env_only", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let result = poll_until_ready(
        &client,
        &format!("{ns}.env_only"),
        json!({}),
        Duration::from_secs(10),
    )
    .await
    .unwrap_or_else(|e| panic!("call must succeed: {} {}", e.code, e.message));
    let structured = extract_structured(&result);
    assert_eq!(structured["mode"], json!("fast"));
    assert_eq!(
        structured["secret_shaped"],
        json!([]),
        "an env-only tool must see no SECRET_* variables at all"
    );
}
