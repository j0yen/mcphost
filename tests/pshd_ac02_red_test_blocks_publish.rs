//! PRD-mcphost-push-from-directory
//! AC2 (P0) — Given a directory with `tests/empty.json` whose tool raises on
//! empty args, When `push` runs, Then stdout shows `tests 0/1` with the
//! `tool_exception` line, exit code is 1, and `tools/list` on the host does
//! not contain the tool.

use crate::common;
use common::{McpClient, TestServer, ToolDirFixture, python_kind_registry, tenant_url};
use mcphost::sandbox;
use tokio::process::Command;

#[tokio::test]
async fn pshd_ac02_red_test_exits_1_and_publishes_nothing() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let data = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&data.0)).await;
    let url = tenant_url(&server.base_url).await;
    let dir = ToolDirFixture::from_example("hello-py");
    std::fs::remove_file(dir.0.join("tests/greet.json")).expect("drop the green test");
    dir.write("tool.py", "def main(args):\n    if not args:\n        raise ValueError(\"empty args\")\n    return {\"ok\": True}\n");
    dir.write("tool.toml", "[tool]\nname = \"hello_py\"\nkind = \"python\"\n");
    dir.write("tests/empty.json", "{\"args\": {}}");

    let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .arg("push")
        .arg(&dir.0)
        .args(["--url", &url])
        .env_remove("MCPHOST_URL")
        .output()
        .await
        .expect("run mcphost push");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "stdout: {stdout} stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("tests 0/1"), "{stdout}");
    assert!(stdout.contains("tests/empty.json") && stdout.contains("tool_exception"), "{stdout}");
    assert!(!stdout.contains("/u/"), "the tenant url is never printed: {stdout}");

    let path = url.trim_start_matches(&server.base_url).to_string();
    let listed = McpClient::new(&server.base_url).with_path(&path).tools_list().await.expect("tools/list");
    assert!(!listed.to_string().contains("hello_py"), "red push must not publish: {listed}");
}
