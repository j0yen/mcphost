//! PRD-mcphost-push-from-directory
//! AC5 (P0) — Given the host refuses the publish (`host_not_allowed` for a
//! literal `localhost` url in an http spec), When `push` runs, Then exit code
//! is 2 and the host's `code` and `message` are printed verbatim.

use crate::common;
use common::{McpClient, TestServer, ToolDirFixture, tenant_url};
use mcphost::kinds::KindRegistry;
use mcphost::kinds::http::HttpKind;
use serde_json::json;
use std::sync::Arc;
use tokio::process::Command;

#[tokio::test]
async fn pshd_ac05_host_refusal_exits_2_and_prints_code_and_message() {
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(HttpKind::new("mcphost.dev").expect("http kind")));
    let server = TestServer::start_with_kinds(kinds).await;
    let url = tenant_url(&server.base_url).await;
    let dir = ToolDirFixture::from_example("hello");
    std::fs::remove_dir_all(dir.0.join("tests")).expect("no tests");
    dir.write(
        "tool.toml",
        "[tool]\nname = \"hook\"\nkind = \"http\"\n\n[spec]\nmethod = \"POST\"\nurl = \"http://localhost/hook\"\n\n[spec.args_schema]\ntype = \"object\"\n",
    );

    // What the host itself says to the same publish.
    let path = url.trim_start_matches(&server.base_url).to_string();
    let direct = McpClient::new(&server.base_url)
        .with_path(&path)
        .tools_call(
            "host.tool.publish",
            json!({"name": "hook", "kind": "http", "spec": {"method": "POST", "url": "http://localhost/hook", "args_schema": {"type": "object"}}}),
        )
        .await
        .expect_err("the host refuses a literal localhost url");
    assert_eq!(direct.error_code.as_deref(), Some("host_not_allowed"));

    let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .arg("push")
        .arg(&dir.0)
        .args(["--url", &url])
        .env_remove("MCPHOST_URL")
        .output()
        .await
        .expect("run mcphost push");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(2), "stdout: {stdout} stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("host_not_allowed"), "{stdout}");
    assert!(stdout.contains(&direct.message), "the host's message is printed verbatim: {stdout} vs {}", direct.message);
}
