//! PRD-mcphost-trusted-discovery-catalog-receipts AC7 (P0) -- Given an in-process
//! `TestServer`, When `tests/live/tdisc_live_receipts_rank.py` runs under
//! `python3 -I` against `<base_url>/mcp`, Then its last stdout line is
//! `tdisc_live first=tested receipts=ok hidden=2 view=real offboarded rc=0`, it
//! exits 0, both tenants it signed up are offboarded and no key is printed;
//! against a closed loopback port its last line ends `rc=2` and it exits 2.

use crate::common;

use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, python_kind_registry};
use mcphost::sandbox;
use serde_json::json;
use std::path::PathBuf;
use std::process::{Command, Output};

/// The display names the driver signs up under are `synthorg-tdisc-live-{a,b}-<run id>`.
const SIGNUP_PREFIX: &str = "synthorg-tdisc-live-";

fn driver() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/live/tdisc_live_receipts_rank.py")
}

/// Runs the driver off the runtime thread (it blocks on HTTP to the in-process server).
async fn run_driver(url: String) -> Output {
    tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new("python3");
        cmd.args(["-I", "-B"]).arg(driver()).arg(url).env("NO_PROXY", "127.0.0.1").env("no_proxy", "127.0.0.1");
        common::strip_cargo_injected_env(&mut cmd);
        cmd.output().expect("python3 runs")
    })
    .await
    .expect("driver task joins")
}

/// True when `text` holds a run of 64 hex digits -- the shape of a tenant key.
fn holds_a_key(text: &str) -> bool {
    let mut run = 0;
    for c in text.chars() {
        run = if c.is_ascii_hexdigit() { run + 1 } else { 0 };
        if run >= 64 {
            return true;
        }
    }
    false
}

#[tokio::test]
async fn tdisc_ac07() {
    // The driver publishes python tools: the same user-namespace guard as every python-kind test.
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let out = run_driver(format!("{}/mcp", server.base_url)).await;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        stdout.lines().last(),
        Some("tdisc_live first=tested receipts=ok hidden=2 view=real offboarded rc=0"),
        "stdout: {stdout}; stderr: {stderr}"
    );
    assert_eq!(out.status.code(), Some(0), "stderr: {stderr}");
    assert!(!holds_a_key(&stdout) && !holds_a_key(&stderr), "a key reached the output: {stdout} {stderr}");

    // both tenants are offboarded
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let listing = extract_structured(&admin.tools_call("admin.tenants", json!({})).await.expect("admin.tenants"));
    let mine: Vec<_> = listing["tenants"]
        .as_array()
        .expect("tenants array")
        .iter()
        .filter(|t| t["display_name"].as_str().is_some_and(|n| n.starts_with(SIGNUP_PREFIX)))
        .collect();
    assert_eq!(mine.len(), 2, "the driver signs up exactly two tenants: {listing}");
    for row in &mine {
        assert_eq!(row["disabled"], json!(true), "the driver's tenant is still active: {row}");
    }

    // a closed loopback port: nothing listens, so the driver fails closed with rc=2
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        listener.local_addr().expect("local addr").port()
    };
    let dead = run_driver(format!("http://127.0.0.1:{port}/mcp")).await;
    let dead_out = String::from_utf8_lossy(&dead.stdout);
    let last = dead_out.lines().last().unwrap_or_default();
    assert!(last.ends_with("rc=2"), "stdout: {dead_out}; stderr: {}", String::from_utf8_lossy(&dead.stderr));
    assert!(last.starts_with("tdisc_live "), "{last}");
    assert_eq!(dead.status.code(), Some(2));
}
