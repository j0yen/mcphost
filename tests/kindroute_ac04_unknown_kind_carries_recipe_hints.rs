//! PRD-mcphost-unknown-kind-routes-to-recipe
//! AC4 (P0) -- Given `host.tool_publish` with `kind: "lambda"`, When the
//! server responds, Then the JSON-RPC code is `-32602`, `data.error_code`
//! is `unknown_kind`, `data.registered` lists the five kinds,
//! `data.aliases` lists the aliases, `data.did_you_mean` contains
//! `python`, and `data.docs` is `host.quickstart`.

use std::sync::Arc;

use crate::common;
use common::{McpClient, TempDataDir, TestServer, signup};
use mcphost::kinds::KindRegistry;
use mcphost::kinds::chain::ChainKind;
use mcphost::kinds::http::HttpKind;
use mcphost::kinds::python::PythonKind;
use mcphost::kinds::wasm::WasmKind;
use serde_json::json;

/// Every kind `main.rs` registers in production (requirement 1's technical
/// consideration: "registered kinds live in src/main.rs (chain, echo,
/// http, python, wasm)") -- AC4 needs exactly this set for `data.registered`
/// to name "the five kinds".
fn five_kinds_registry(data_dir: &std::path::Path) -> KindRegistry {
    let mut kinds = KindRegistry::with_builtin(); // echo
    let lookup: Arc<dyn mcphost::kinds::http::NameLookup> =
        Arc::new(common::FixedLookup(Default::default()));
    kinds.register(Arc::new(HttpKind::for_test("127.0.0.1", lookup)));
    kinds.register(Arc::new(PythonKind::new(data_dir)));
    kinds.register(Arc::new(ChainKind));
    kinds.register(Arc::new(WasmKind::new()));
    kinds
}

#[tokio::test]
async fn tool_publish_unknown_kind_carries_registered_aliases_and_did_you_mean() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(five_kinds_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Kindroute AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "my_tool", "kind": "lambda", "spec": {}}),
        )
        .await
        .expect_err("lambda is not a registered kind or alias");

    assert_eq!(err.code, -32602, "unknown_kind must stay INVALID_PARAMS: {err:?}");
    assert_eq!(err.error_code.as_deref(), Some("unknown_kind"));

    let data = &err.data;
    let registered: Vec<&str> = data["registered"]
        .as_array()
        .expect("registered array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    for k in ["chain", "echo", "http", "python", "wasm"] {
        assert!(registered.contains(&k), "registered missing {k}: {registered:?}");
    }
    assert_eq!(registered.len(), 5, "registered: {registered:?}");

    let aliases: Vec<&str> = data["aliases"]
        .as_array()
        .expect("aliases array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    for a in ["event", "webhook", "cron", "schedule"] {
        assert!(aliases.contains(&a), "aliases missing {a}: {aliases:?}");
    }

    let did_you_mean: Vec<&str> = data["did_you_mean"]
        .as_array()
        .expect("did_you_mean array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        did_you_mean.contains(&"python"),
        "did_you_mean must contain python for 'lambda': {did_you_mean:?}"
    );

    assert_eq!(data["docs"], json!("host.quickstart"));
}
