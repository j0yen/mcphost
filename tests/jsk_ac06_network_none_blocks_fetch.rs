//! mcphost-js-kind AC6 (P0) — Given `network: none` and `fetch("https://example.com")`, When
//! called, Then the error names the network grant and no socket opens (bwrap
//! namespace, same test shape as python's egress tests).

use crate::common;
use common::{TestServer, js_kind_registry, signup};
use mcphost::kinds::js::{self, CallParams};
use mcphost::sandbox::{self, IsolationMechanism, NetworkMode, build_isolated_command};
use serde_json::json;
use std::path::Path;

fn argv(cmd: &tokio::process::Command) -> Vec<String> {
    cmd.as_std().get_args().map(|a| a.to_string_lossy().to_string()).collect()
}

#[tokio::test]
async fn fetch_under_network_none_fails_naming_the_network_grant() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(js_kind_registry(&dir.0)).await;
    let (ns, key) = signup(&server.base_url, "JSK AC6 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "async function main(args) {\n  const r = await fetch(\"https://example.com\");\n  return { status: r.status };\n}\n",
        "args_schema": {"type": "object"},
        "network": "none",
    });
    client
        .tools_call("host.tool_publish", json!({"name": "dialer", "kind": "js", "spec": spec}))
        .await
        .expect("publish ok");

    let err = client
        .tools_call(&format!("{ns}.dialer"), json!({}))
        .await
        .expect_err("fetch under network:none must not succeed");
    assert_eq!(err.error_code.as_deref(), Some("tool_exception"), "{err:?}");
    assert!(err.message.contains("network grant"), "message must name the network grant: {}", err.message);
    assert!(err.message.contains("network: none"), "message must name the spec value: {}", err.message);
    assert_eq!(err.data["source_line"], json!("const r = await fetch(\"https://example.com\");"), "{err:?}");
}

#[test]
fn the_js_run_spec_for_network_none_unshares_every_namespace_and_never_shares_net() {
    let params = CallParams {
        source: "function main() {}",
        timeout_s: 10,
        memory_mb: 256,
        network: NetworkMode::None,
        extra_env: Vec::new(),
        isolation: IsolationMechanism::Bwrap,
    };
    let spec = js::run_spec(&params, Path::new("/tmp/jsk-ac06-scratch"), Path::new("/usr/bin/mcphost-qjs"));
    let args = argv(&build_isolated_command(&spec));
    assert!(args.iter().any(|a| a == "--unshare-all"), "{args:?}");
    assert!(!args.iter().any(|a| a == "--share-net"), "{args:?}");

    let unshare = CallParams { isolation: IsolationMechanism::UnshareSetpriv, ..params };
    let args = argv(&build_isolated_command(&js::run_spec(
        &unshare,
        Path::new("/tmp/jsk-ac06-scratch"),
        Path::new("/usr/bin/mcphost-qjs"),
    )));
    assert!(args.iter().any(|a| a == "-n"), "{args:?}");
}
