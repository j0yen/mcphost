//! PRD-mcphost-sandbox-egress-allowlist
//! AC4 (P0) — Given a Pro tenant and `MCPHOST_EGRESS_PROXY=http://
//! 127.0.0.1:3128`, When it calls the egress tool, Then the sandbox command
//! line contains `--share-net` (or the fallback omits `-n`) and the child
//! environment contains `https_proxy=http://127.0.0.1:3128`.
//!
//! `pro_tenant_calling_an_egress_tool_sees_the_configured_proxy_in_its_env`
//! below is the actual Given/When/Then: a real Pro tenant, the real
//! `$MCPHOST_EGRESS_PROXY` env var, a real `host.tool_call` -- so it goes
//! through `PythonKind::network_mode` (src/kinds/python.rs), the only code
//! that reads the env var and gates on `ctx.egress_allowed`, not a
//! hand-built `RunSpec`. The two `#[test]`s below it are a narrower,
//! additional proof of the same AC's command-line wording (`--share-net`/
//! `-n`) at the `sandbox::build_isolated_command` level directly (`RunSpec`
//! in, `tokio::process::Command` out, never spawned) -- kept because
//! that level is the only place the exact bwrap/unshare argv shape can be
//! asserted without parsing a spawned child's real argv.

use crate::common;
use common::{ADMIN_KEY, McpClient, TempDataDir, TestServer, python_kind_registry, signup};
use mcphost::sandbox::{
    self, IsolationMechanism, NetworkMode, ResourceLimits, RunSpec, build_isolated_command,
};
use serde_json::json;
use std::path::PathBuf;
use std::time::Duration;

use crate::egress_proxy_lock;

const PROXY: &str = "http://127.0.0.1:3128";

#[tokio::test]
async fn pro_tenant_calling_an_egress_tool_sees_the_configured_proxy_in_its_env() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    // See egress_proxy_lock's doc comment: serializes this test against
    // the other files in this PRD that also mutate the process-wide
    // MCPHOST_EGRESS_PROXY env var.
    let _guard = egress_proxy_lock::guard().await;
    // SAFETY: held across this whole test body via the async guard above,
    // so no other test in this binary observes a torn env var.
    unsafe {
        std::env::set_var("MCPHOST_EGRESS_PROXY", PROXY);
    }

    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC4 Pro Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    admin
        .tools_call("admin.plan_set", json!({"tenant": ns, "plan": "pro", "reason": "AC4 setup"}))
        .await
        .expect("admin.plan_set to pro");

    let spec = json!({
        "source": "import os\ndef main(args):\n    return {\"https_proxy\": os.environ.get(\"https_proxy\")}\n",
        "args_schema": {"type": "object"},
        "network": "egress",
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "egress_probe", "kind": "python", "spec": spec}),
        )
        .await
        .expect("pro tenant publish of network: egress must succeed");

    let result = client
        .tools_call("host.tool_call", json!({"name": "egress_probe", "args": {}}))
        .await
        .unwrap_or_else(|e| panic!("pro tenant with proxy configured must succeed: {} {}", e.code, e.message));

    let structured = common::extract_structured(&result);
    assert_eq!(
        structured["https_proxy"],
        json!(PROXY),
        "the sandboxed child must see the configured proxy as https_proxy: {structured}"
    );

    unsafe {
        std::env::remove_var("MCPHOST_EGRESS_PROXY");
    }
}

fn base_spec(isolation: IsolationMechanism) -> RunSpec {
    RunSpec {
        interpreter: PathBuf::from("/usr/bin/python3"),
        interpreter_args: vec!["-I".to_string(), "-S".to_string()],
        script_path: PathBuf::from("/tmp/does-not-need-to-exist/runner.py"),
        scratch_dir: PathBuf::from("/tmp/does-not-need-to-exist"),
        read_only_dirs: vec![],
        stdin_payload: Vec::new(),
        limits: ResourceLimits {
            cpu_seconds: 5,
            memory_mb: 256,
            max_open_files: 64,
            max_file_size_mb: 16,
            max_processes: 64,
        },
        wall_clock_timeout: Duration::from_secs(7),
        network: NetworkMode::Public {
            http_proxy: Some(PROXY.to_string()),
        },
        extra_env: vec![],
        isolation,
    }
}

fn argv(cmd: &tokio::process::Command) -> Vec<String> {
    cmd.as_std().get_args().map(|a| a.to_string_lossy().to_string()).collect()
}

#[test]
fn bwrap_command_shares_net_and_sets_https_proxy() {
    let cmd = build_isolated_command(&base_spec(IsolationMechanism::Bwrap));
    let args = argv(&cmd);
    assert!(args.iter().any(|a| a == "--share-net"), "{args:?}");
    // bwrap's env is set via `--setenv <key> <value>` argv pairs, not the
    // process's own env -- find the pair naming `https_proxy`.
    let https_proxy_value = args
        .windows(3)
        .find(|w| w[0] == "--setenv" && w[1] == "https_proxy")
        .map(|w| w[2].clone());
    assert_eq!(https_proxy_value.as_deref(), Some(PROXY), "{args:?}");
}

#[test]
fn unshare_fallback_omits_dash_n_and_sets_https_proxy() {
    let cmd = build_isolated_command(&base_spec(IsolationMechanism::UnshareSetpriv));
    let args = argv(&cmd);
    assert!(!args.iter().any(|a| a == "-n"), "{args:?}");
    let https_proxy_value = cmd
        .as_std()
        .get_envs()
        .find(|(k, _)| *k == "https_proxy")
        .and_then(|(_, v)| v)
        .map(|v| v.to_string_lossy().to_string());
    assert_eq!(https_proxy_value.as_deref(), Some(PROXY));
}
