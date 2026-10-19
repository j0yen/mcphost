//! PRD-mcphost-push-from-directory
//! AC1 (P0) — Given a fixture host and `examples/tools/hello-py/` pushed
//! twice without edits, When the second push runs, Then it prints
//! `unchanged` and `host.tool.history` holds exactly one version.

use crate::common;
use common::{McpClient, TestServer, ToolDirFixture, extract_structured, python_kind_registry, tenant_url};
use mcphost::sandbox;
use serde_json::json;
use tokio::process::Command;

async fn push(dir: &std::path::Path, url: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .arg("push")
        .arg(dir)
        .args(["--url", url])
        .env_remove("MCPHOST_URL")
        .output()
        .await
        .expect("run mcphost push")
}

#[tokio::test]
async fn pshd_ac01_second_push_prints_unchanged_and_history_keeps_one_version() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let data = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&data.0)).await;
    let url = tenant_url(&server.base_url).await;
    let dir = ToolDirFixture::from_example("hello-py");

    let first = push(&dir.0, &url).await;
    assert_eq!(first.status.code(), Some(0), "stdout: {} stderr: {}", String::from_utf8_lossy(&first.stdout), String::from_utf8_lossy(&first.stderr));
    assert!(String::from_utf8_lossy(&first.stdout).contains("published"), "{}", String::from_utf8_lossy(&first.stdout));

    let second = push(&dir.0, &url).await;
    assert_eq!(second.status.code(), Some(0));
    let out = String::from_utf8_lossy(&second.stdout);
    assert!(out.contains("unchanged"), "second push must print unchanged: {out}");

    let path = url.trim_start_matches(&server.base_url).to_string();
    let client = McpClient::new(&server.base_url).with_path(&path);
    let history = client.tools_call("host.tool.history", json!({"name": "hello_py"})).await.expect("history");
    let versions = extract_structured(&history)["versions"].as_array().expect("versions").len();
    assert_eq!(versions, 1, "an unchanged push writes no version row");
}
