//! PRD-mcphost-push-from-directory
//! AC3 (P0) — Given a directory with three green tests, When `push --json`
//! runs, Then the JSON carries `tool`, `version`, `tests.ok = 3`,
//! `call_line`, and `proof` (`none` when `host.demo_url` is not listed).

use crate::common;
use common::{TestServer, ToolDirFixture, python_kind_registry, tenant_url};
use mcphost::sandbox;
use serde_json::{Value, json};
use tokio::process::Command;

#[tokio::test]
async fn pshd_ac03_json_carries_tool_version_tests_call_line_and_proof() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let data = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&data.0)).await;
    let url = tenant_url(&server.base_url).await;
    let dir = ToolDirFixture::from_example("hello-py");
    dir.write("tests/ada.json", r#"{"args": {"name": "ada"}, "expect": {"greeting": "hello ada"}}"#);
    dir.write("tests/bob.json", r#"{"args": {"name": "bob"}}"#);

    let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .arg("push")
        .arg(&dir.0)
        .args(["--json", "--url", &url])
        .env_remove("MCPHOST_URL")
        .output()
        .await
        .expect("run mcphost push");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "stdout: {stdout} stderr: {}", String::from_utf8_lossy(&out.stderr));
    let v: Value = serde_json::from_str(stdout.trim()).expect("one JSON object on stdout");
    assert_eq!(v["tool"], json!("hello_py"));
    assert_eq!(v["version"], json!(1));
    assert_eq!(v["tests"]["ok"], json!(3));
    // Tests run in file-name order; the first is ada.json.
    assert_eq!(v["call_line"], json!(r#"host.tool_call(name="hello_py", args={"name":"ada"})"#));
    assert_eq!(v["proof"], json!("none"), "this host lists no host.demo_url");
    assert!(!stdout.contains("/u/"), "the tenant url is never printed: {stdout}");
}
