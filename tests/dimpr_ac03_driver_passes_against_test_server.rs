//! PRD-mcphost-docs-put-many-live-reproof
//! AC3 -- Given an in-process `TestServer`, When the driver runs under
//! `python3 -I` against `<base_url>/mcp`, Then its last stdout line is
//! `dimp_ac08 stored=3 hits=1 offboarded rc=0`, it exits 0, and afterwards the
//! server reports no active tenant for the driver's synthetic signup
//! (offboarded or absent), read through `admin.tenants`; the test never reads
//! the tenant key (the driver holds it in memory only).

use crate::common;

use common::{ADMIN_KEY, McpClient, TestServer, extract_structured};
use serde_json::json;
use std::path::PathBuf;
use std::process::Command;

/// The display name the driver signs up under is `synthorg-dimp-ac08-<run id>`.
const SIGNUP_PREFIX: &str = "synthorg-dimp-ac08-";

#[tokio::test]
async fn dimpr_ac03_driver_passes_against_the_test_server_and_leaves_no_active_tenant() {
    let server = TestServer::start().await;
    let url = format!("{}/mcp", server.base_url);
    let driver = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/live/dimp_ac08_put_many.py");

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
    assert_eq!(
        stdout.lines().last(),
        Some("dimp_ac08 stored=3 hits=1 offboarded rc=0"),
        "stdout: {stdout}; stderr: {stderr}"
    );
    assert_eq!(out.status.code(), Some(0), "stderr: {stderr}");

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let listing = extract_structured(&admin.tools_call("admin.tenants", json!({})).await.expect("admin.tenants"));
    let mine: Vec<_> = listing["tenants"]
        .as_array()
        .expect("tenants array")
        .iter()
        .filter(|t| t["display_name"].as_str().is_some_and(|n| n.starts_with(SIGNUP_PREFIX)))
        .collect();
    // The test server never purges, so the signup is listed; were the listing to
    // drop it the loop below would pass vacuously, which AC3 also allows ("absent").
    assert!(!mine.is_empty(), "no row for the driver's signup in {listing}");
    for row in &mine {
        assert_eq!(row["disabled"], json!(true), "the driver's tenant is still active: {row}");
    }
}
