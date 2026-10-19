//! PRD-mcphost-push-from-directory
//! AC4 (P0) — Given `--signup` on a fixture host, When `push` completes,
//! Then the tenant URL appears only in the 0600 config file and stdout
//! contains the claim link and tenant id but no `/u/` path.

use crate::common;
use common::{TempDataDir, TestServer, ToolDirFixture};
use std::os::unix::fs::PermissionsExt;
use tokio::process::Command;

#[tokio::test]
async fn pshd_ac04_signup_writes_the_url_to_a_0600_file_and_prints_only_claim_and_tenant() {
    let server = TestServer::start().await;
    let dir = ToolDirFixture::from_example("hello");
    let config = TempDataDir::new();

    let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .arg("push")
        .arg(&dir.0)
        .args(["--signup", "--url", &server.base_url])
        .env_remove("MCPHOST_URL")
        .env("XDG_CONFIG_HOME", &config.0)
        .output()
        .await
        .expect("run mcphost push --signup");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "stdout: {stdout} stderr: {stderr}");

    let file = config.0.join("mcphost/tenant.url");
    let url = std::fs::read_to_string(&file).expect("tenant.url written");
    let url = url.trim();
    assert!(url.starts_with(&format!("{}/u/", server.base_url)), "config file holds the tenant url: {url}");
    assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);

    assert!(!stdout.contains("/u/") && !stderr.contains("/u/"), "no /u/ path on any stream: {stdout}{stderr}");
    let secret = url.split("/u/").nth(1).unwrap().trim_end_matches("/mcp");
    assert!(!stdout.contains(secret) && !stderr.contains(secret), "the url secret never leaves the config file");
    assert!(stdout.contains("tenant t_"), "stdout names the tenant id: {stdout}");
    assert!(stdout.contains("claim: http"), "stdout carries the claim link: {stdout}");
    assert!(stdout.contains("published hello"), "{stdout}");
}
