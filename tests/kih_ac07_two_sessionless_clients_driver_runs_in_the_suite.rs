//! PRD-mcphost-implicit-signup-key-in-hand
//! AC7 (P0) -- Given this PRD's tree, When a `kih` Rust test spawns the
//! in-process host and runs `python3 -I
//! tests/live/kih_ac08_two_sessionless_clients.py <its /mcp URL>`, Then the
//! driver exits 0 with last line `kih_ac08 A=offboarded B=offboarded rc=0`,
//! both tenants it minted are offboarded, and the driver file is committed in
//! the same change (the predecessor's AC 8 driver never was).

use crate::common;

use common::{ADMIN_KEY, McpClient, TestServer, extract_structured};
use serde_json::json;
use std::path::PathBuf;
use std::process::Command;

const DRIVER: &str = "tests/live/kih_ac08_two_sessionless_clients.py";

#[tokio::test]
async fn kih_ac07() {
    let server = TestServer::start().await;
    let url = format!("{}/mcp", server.base_url);
    let driver = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(DRIVER);
    assert!(driver.is_file(), "the driver is a committed file: {}", driver.display());

    // The driver blocks on HTTP to this very server, so it runs off the runtime thread.
    let out = tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new("python3");
        cmd.args(["-I", "-B"]).arg(driver).arg(url).env("NO_PROXY", "127.0.0.1").env("no_proxy", "127.0.0.1");
        common::strip_cargo_injected_env(&mut cmd);
        cmd.output().expect("python3 runs")
    })
    .await
    .expect("driver task joins");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(stdout.lines().last(), Some("kih_ac08 A=offboarded B=offboarded rc=0"), "stdout: {stdout}; stderr: {stderr}");
    assert_eq!(out.status.code(), Some(0), "stderr: {stderr}");
    assert!(!stdout.contains("tenant_key") && !stderr.contains("tenant_key"), "no key material is printed");

    let tenants = server.state.db.list_tenants().await.expect("tenants");
    assert_eq!(tenants.len(), 2, "the driver minted exactly two tenants");
    for t in &tenants {
        assert!(t.disabled, "{} is offboarded", t.namespace);
        assert_eq!(t.disabled_reason.as_deref(), Some("self_offboard"));
        assert!(!stdout.contains(&t.namespace) && !stderr.contains(&t.namespace), "no namespace is printed");
    }
    // Read through the admin surface too: both rows are disabled.
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let listing = extract_structured(&admin.tools_call("admin.tenants", json!({})).await.expect("admin.tenants"));
    let rows = listing["tenants"].as_array().expect("tenants array");
    assert_eq!(rows.iter().filter(|t| t["disabled"] == json!(true)).count(), 2, "{listing}");
}
