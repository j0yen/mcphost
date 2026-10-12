//! AC3 (PRD-mcphost-call-cli-from-contract) -- Given a running test host and
//! `MCPHOST_KEY` set, When `mcphost call host.tool.publish --name hello
//! --kind python --spec-json @fixture.json --json` runs, Then stdout is one
//! JSON line equal to the host's `structuredContent` and the key never
//! appears in argv, stdout or stderr.
//!
//! The real-host test publishes an `echo` tool (a `python` publish needs the
//! userns sandbox; the CLI path under test -- flag typing, `@file`, bearer,
//! output -- is identical for every kind). The stub-host test pins the
//! equality with `structuredContent` and the wire shape exactly.

use crate::common;

use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};
use tokio::process::Command;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const FIXTURE: &str = r#"{"schema": {"type": "object", "properties": {"msg": {"type": "string"}}, "required": ["msg"]}}"#;

fn fixture_file(tag: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("ccli_ac03_{tag}_{}.json", std::process::id()));
    std::fs::write(&path, FIXTURE).expect("write fixture");
    path
}

async fn mcphost_call(args: &[&str], key: Option<&str>) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mcphost"));
    cmd.arg("call").args(args).env_remove("MCPHOST_KEY");
    if let Some(key) = key {
        cmd.env("MCPHOST_KEY", key);
    }
    cmd.output().await.expect("run mcphost call")
}

fn assert_key_absent(key: &str, argv: &[&str], out: &std::process::Output) {
    assert!(!argv.iter().any(|a| a.contains(key)), "key leaked into argv");
    assert!(!String::from_utf8_lossy(&out.stdout).contains(key), "key leaked into stdout");
    assert!(!String::from_utf8_lossy(&out.stderr).contains(key), "key leaked into stderr");
}

#[tokio::test]
async fn publish_through_the_cli_lands_on_a_real_host_and_the_key_stays_off_every_stream() {
    let server = TestServer::start().await;
    let (tenant, key) = signup(&server.base_url, "Cli Publisher").await;
    let fixture = fixture_file("real");
    let spec = format!("@{}", fixture.display());

    let argv = [
        "host.tool.publish", "--name", "hello", "--kind", "echo", "--spec-json", &spec, "--json",
        "--no-skew-check", "--url", &server.base_url,
    ];
    let out = mcphost_call(&argv, Some(&key)).await;
    assert_eq!(out.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_key_absent(&key, &argv, &out);

    let stdout = String::from_utf8(out.stdout.clone()).expect("utf8 stdout");
    assert_eq!(stdout.trim_end().lines().count(), 1, "--json prints exactly one line: {stdout}");
    let printed: Value = serde_json::from_str(stdout.trim()).expect("stdout is JSON");
    assert_eq!(printed["name"], format!("{tenant}.hello"));

    // The host really holds the tool the CLI published.
    let client = McpClient::with_bearer(&server.base_url, &key);
    let listed = client.tools_call("host.tool.list", json!({})).await.expect("host.tool.list");
    assert!(
        extract_structured(&listed).to_string().contains("hello"),
        "published tool is listed"
    );
}

#[tokio::test]
async fn stdout_equals_structured_content_and_the_key_travels_only_as_a_bearer_header() {
    let structured = json!({"name": "t_abc.hello", "kind": "echo", "version": 1, "nested": {"a": [1, 2]}});
    let host = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "content": [{"type": "text", "text": structured.to_string()}],
                "structuredContent": structured,
            }
        })))
        .mount(&host)
        .await;

    let key = "mcp_secret_key_value_8f3a";
    let fixture = fixture_file("stub");
    let spec = format!("@{}", fixture.display());
    let argv = [
        "host.tool.publish", "--name", "hello", "--kind", "echo", "--spec-json", &spec, "--json",
        "--no-skew-check", "--url", &host.uri(),
    ];
    let out = mcphost_call(&argv, Some(key)).await;
    assert_eq!(out.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_key_absent(key, &argv, &out);

    let stdout = String::from_utf8(out.stdout).expect("utf8 stdout");
    assert_eq!(stdout.trim_end().lines().count(), 1, "one JSON line: {stdout}");
    assert_eq!(serde_json::from_str::<Value>(stdout.trim()).unwrap(), structured);

    let requests = host.received_requests().await.expect("recording on");
    let call = requests.iter().find(|r| r.method.as_str() == "POST").expect("one POST /mcp");
    assert_eq!(
        call.headers.get("authorization").and_then(|v| v.to_str().ok()),
        Some(format!("Bearer {key}").as_str())
    );
    let body: Value = serde_json::from_slice(&call.body).expect("json-rpc body");
    assert_eq!(body["method"], "tools/call");
    assert_eq!(body["params"]["name"], "host.tool.publish");
    assert_eq!(
        body["params"]["arguments"],
        json!({"name": "hello", "kind": "echo", "spec": serde_json::from_str::<Value>(FIXTURE).unwrap()})
    );
    assert!(
        body["params"]["arguments"].get("tenant_key").is_none(),
        "the key is never an argument"
    );

    // --key-file is the other way in, and equally silent.
    let key_file = std::env::temp_dir().join(format!("ccli_ac03_key_{}", std::process::id()));
    std::fs::write(&key_file, format!("{key}\n")).expect("write key file");
    let kf = key_file.display().to_string();
    let argv = [
        "host.tool.publish", "--name", "hello", "--kind", "echo", "--spec-json", &spec, "--json",
        "--no-skew-check", "--key-file", &kf, "--url", &host.uri(),
    ];
    let out = mcphost_call(&argv, None).await;
    assert_eq!(out.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_key_absent(key, &argv, &out);
    let requests = host.received_requests().await.expect("recording on");
    let last = requests.iter().rfind(|r| r.method.as_str() == "POST").expect("second POST");
    assert_eq!(
        last.headers.get("authorization").and_then(|v| v.to_str().ok()),
        Some(format!("Bearer {key}").as_str())
    );
}
