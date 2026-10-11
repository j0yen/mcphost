//! PRD-mcphost-docs-examples-executed-nightly AC4 (P0) -- Given an example
//! tagged `needs=anonymous`, When run, Then it is sent without a key; Given
//! `skip=needs-secret`, Then it prints `skip` and is counted in `skipped`,
//! not in `examples_failed`.
//!
//! The host here is a recording stub (every POST /mcp answers with a
//! successful result carrying a `key`), so the test can read exactly which
//! requests carried an `Authorization` header.

use std::path::PathBuf;

use serde_json::Value;
use tokio::process::Command;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PAGE: &str = "# Auth\n\n## Modes\n\n```json mcphost-call needs=anonymous\n{\"name\": \"host.quickstart\", \"arguments\": {}}\n```\n\n```json mcphost-call\n{\"name\": \"host.whoami\", \"arguments\": {}}\n```\n\n```json mcphost-call skip=needs-secret\n{\"name\": \"host.secret_set\", \"arguments\": {\"name\": \"stripe\", \"value\": \"x\"}}\n```\n";

#[tokio::test]
async fn anonymous_examples_carry_no_key_and_skips_are_counted_apart_from_failures() {
    let host = MockServer::start().await;
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "result": {"content": [{"type": "text", "text": "{\"tenant\":\"t\",\"key\":\"k-docs-example\"}"}]}
    });
    Mock::given(method("POST")).and(path("/mcp")).respond_with(ResponseTemplate::new(200).set_body_json(body)).mount(&host).await;

    let root: PathBuf = std::env::temp_dir().join(format!("dxn-ac04-{}-{}", std::process::id(), mcphost::state::now_unix_ms()));
    std::fs::create_dir_all(root.join("docs")).unwrap();
    std::fs::write(root.join("docs/page.md"), PAGE).unwrap();
    let bin = env!("CARGO_BIN_EXE_mcphost");
    let extract = Command::new(bin).args(["docs-examples", "extract", "--root"]).arg(&root).output().await.unwrap();
    assert!(extract.status.success(), "{}", String::from_utf8_lossy(&extract.stderr));

    let out = Command::new(bin)
        .args(["docs-examples", "run", "--url", &host.uri(), "--root"])
        .arg(&root)
        .arg("--artifact-dir")
        .arg(root.join("artifacts"))
        .env_remove("MCPHOST_TENANT_KEY")
        .output()
        .await
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "run:\n{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("docs/page.md#modes-3 skip needs-secret"), "skip line:\n{stdout}");
    assert!(stdout.contains("examples_total=3 examples_failed=0 skipped=1"), "summary:\n{stdout}");

    let mut seen = std::collections::BTreeMap::new();
    for req in host.received_requests().await.unwrap() {
        let json: Value = serde_json::from_slice(&req.body).unwrap();
        let name = json["params"]["name"].as_str().unwrap_or(json["method"].as_str().unwrap_or("")).to_string();
        let auth = req.headers.get("authorization").map(|v| v.to_str().unwrap().to_string());
        seen.insert(name, auth);
    }
    assert_eq!(seen["host.quickstart"], None, "needs=anonymous must be sent without a key: {seen:?}");
    assert_eq!(seen["host.whoami"].as_deref(), Some("Bearer k-docs-example"), "default needs=tenant carries the key: {seen:?}");
    assert!(!seen.contains_key("host.secret_set"), "a skipped example must not be sent: {seen:?}");
    std::fs::remove_dir_all(&root).ok();
}
