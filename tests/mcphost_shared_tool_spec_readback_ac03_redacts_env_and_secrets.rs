//! PRD-mcphost-shared-tool-spec-readback
//! AC3 (P0) — Given a tool whose spec has `env: {"API_KEY": ...}` and a
//! secret reference (`secrets: [...]`) in its config, When a sharee reads
//! it via `host.tool_spec_shared`, Then the response contains no `env`
//! key, no `secrets` key, and no secret-reference string, while `source`,
//! `args_schema`, `requirements`, `timeout_s`, `network` do appear.

use crate::common;
use common::{McpClient, TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn sharee_response_never_carries_env_or_secret_reference() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;

    let (ns_o, key_o) = signup(&server.base_url, "AC3 Owner").await;
    let client_o = McpClient::with_bearer(&server.base_url, &key_o);
    client_o
        .tools_call("host.group.create", json!({"name": "builders"}))
        .await
        .expect("group.create");

    let (ns_m, key_m) = signup(&server.base_url, "AC3 Member").await;
    client_o
        .tools_call("host.group.add", json!({"name": "builders", "namespace": ns_m}))
        .await
        .expect("group.add");

    let secret_value = "sk_live_super_secret_9f31";
    client_o
        .tools_call("host.secret_set", json!({"name": "api_key", "value": secret_value}))
        .await
        .expect("secret_set ok");

    let source = "def main(args):\n    return {\"ok\": True}\n";
    let spec = json!({
        "source": source,
        "args_schema": {"type": "object"},
        "requirements": [],
        "timeout_s": 12,
        "network": "none",
        "env": {"MODE": "fast", "REGION": "us-east"},
        "secrets": ["api_key"],
    });
    client_o
        .tools_call(
            "host.tool_publish",
            json!({"name": "priced_call", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");
    client_o
        .tools_call(
            "host.tool_share",
            json!({
                "name": "priced_call",
                "visibility": "group",
                "group": "builders",
                "expose_spec": true,
            }),
        )
        .await
        .expect("share ok");

    let client_m = McpClient::with_bearer(&server.base_url, &key_m);
    let result = client_m
        .tools_call(
            "host.tool_spec_shared",
            json!({"tool": format!("{ns_o}.priced_call")}),
        )
        .await
        .expect("tool_spec_shared ok");
    let structured = extract_structured(&result);
    let response_text = structured.to_string();

    let spec_out = structured["spec"].as_object().expect("spec object");
    assert!(!spec_out.contains_key("env"), "spec must omit env entirely: {spec_out:?}");
    assert!(!spec_out.contains_key("secrets"), "spec must omit secrets entirely: {spec_out:?}");
    assert!(
        !response_text.contains(secret_value),
        "response must never carry the secret value: {response_text}"
    );
    assert!(
        !response_text.contains("api_key"),
        "response must never carry the secret reference name: {response_text}"
    );
    assert!(
        !response_text.contains("MODE") && !response_text.contains("REGION"),
        "response must never carry a plain env entry either: {response_text}"
    );

    assert_eq!(spec_out["source"], json!(source));
    assert_eq!(spec_out["args_schema"], json!({"type": "object"}));
    assert_eq!(spec_out["requirements"], json!([]));
    assert_eq!(spec_out["timeout_s"], json!(12));
    assert_eq!(spec_out["network"], json!("none"));
}
