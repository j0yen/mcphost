//! AC4 (PRD-mcphost-call-cli-from-contract) -- Given a host returning
//! `args_invalid`, `bearer_invalid`, `tool_exception` and a 503, When each is
//! received, Then the exit codes are 2, 4, 3, 5 and stderr carries
//! `error_code` and `data`; the mapping test covers every `AppError::code()`
//! arm.

use mcphost::client::{EXIT_CODE_TABLE, exit_code_for};
use serde_json::{Value, json};
use tokio::process::Command;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn rpc_error(code: i64, message: &str, error_code: &str, extra: Value) -> ResponseTemplate {
    let mut data = json!({"error_code": error_code});
    data.as_object_mut().unwrap().extend(extra.as_object().cloned().unwrap_or_default());
    ResponseTemplate::new(200).set_body_json(json!({
        "jsonrpc": "2.0", "id": 1, "error": {"code": code, "message": message, "data": data}
    }))
}

/// Run `mcphost call host.whoami` against a stub that answers every
/// `POST /mcp` with `response`; returns `(exit code, stderr)`.
async fn call_against(response: ResponseTemplate) -> (Option<i32>, String) {
    let host = MockServer::start().await;
    Mock::given(method("POST")).and(path("/mcp")).respond_with(response).mount(&host).await;
    let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["call", "host.whoami", "--no-skew-check", "--url", &host.uri()])
        .env_remove("MCPHOST_KEY")
        .output()
        .await
        .expect("run mcphost call");
    (out.status.code(), String::from_utf8_lossy(&out.stderr).into_owned())
}

fn stderr_envelope(stderr: &str) -> Value {
    let line = stderr.lines().rfind(|l| l.starts_with('{')).unwrap_or_else(|| panic!("no JSON envelope on stderr: {stderr}"));
    serde_json::from_str(line).expect("envelope parses")
}

#[tokio::test]
async fn each_error_class_exits_with_its_code_and_prints_error_code_and_data() {
    let cases: Vec<(&str, ResponseTemplate, i32, Value)> = vec![
        ("args_invalid", rpc_error(-32602, "name is required", "args_invalid", json!({"field": "name"})), 2, json!({"field": "name"})),
        ("bearer_invalid", rpc_error(-32600, "missing or invalid Authorization: Bearer key", "bearer_invalid", json!({"hint": "signup"})), 4, json!({"hint": "signup"})),
        ("tool_exception", rpc_error(-32603, "ZeroDivisionError: division by zero", "tool_exception", json!({"line": 3})), 3, json!({"line": 3})),
    ];
    for (code, response, exit, data) in cases {
        let (got, stderr) = call_against(response).await;
        assert_eq!(got, Some(exit), "{code}: stderr {stderr}");
        let envelope = stderr_envelope(&stderr);
        assert_eq!(envelope["error_code"], code);
        assert!(envelope["message"].as_str().is_some_and(|m| !m.is_empty()), "{code} carries the host message");
        for (k, v) in data.as_object().unwrap() {
            assert_eq!(envelope["data"][k], *v, "{code}: data.{k} on stderr");
        }
    }

    let (got, stderr) = call_against(ResponseTemplate::new(503).set_body_string("upstream down")).await;
    assert_eq!(got, Some(5), "a 503 is the host failing: {stderr}");
    let envelope = stderr_envelope(&stderr);
    assert_eq!(envelope["error_code"], "host_unavailable");
    assert_eq!(envelope["data"]["status"], 503);
}

#[tokio::test]
async fn an_unreachable_host_exits_5() {
    let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["call", "host.whoami", "--no-skew-check", "--url", "http://127.0.0.1:1"])
        .env_remove("MCPHOST_KEY")
        .output()
        .await
        .expect("run mcphost call");
    assert_eq!(out.status.code(), Some(5));
    assert_eq!(stderr_envelope(&String::from_utf8_lossy(&out.stderr))["error_code"], "host_unreachable");
}

/// Every `AppError::code()` arm has an explicit row in the exit-code table:
/// the arms are read from `src/errors.rs` itself, so a new error variant
/// that is not mapped fails here instead of silently exiting 1.
#[test]
fn the_table_names_every_apperror_code_arm() {
    let src = include_str!("../src/errors.rs");
    let start = src.find("pub fn code(&self)").expect("AppError::code");
    let end = start + src[start..].find("fn jsonrpc_code").expect("end of code()");
    let mut arms = Vec::new();
    for line in src[start..end].lines().filter(|l| !l.trim_start().starts_with("//")) {
        if let Some((_, rhs)) = line.split_once("=> \"") {
            arms.push(rhs.split('"').next().unwrap().to_string());
        }
    }
    assert!(arms.len() > 30, "parsed {} code() arms -- the parser drifted from errors.rs", arms.len());
    let missing: Vec<&String> = arms
        .iter()
        .filter(|arm| !EXIT_CODE_TABLE.iter().any(|(code, _)| code == arm))
        .collect();
    assert!(missing.is_empty(), "AppError::code() arms with no exit-code row: {missing:?}");

    // The four codes the PRD pins, and a code the table has never seen.
    assert_eq!(exit_code_for("args_invalid"), 2);
    assert_eq!(exit_code_for("invalid_params"), 2);
    assert_eq!(exit_code_for("bearer_invalid"), 4);
    assert_eq!(exit_code_for("tool_exception"), 3);
    assert_eq!(exit_code_for("service_unavailable"), 5);
    assert_eq!(exit_code_for("a_code_from_the_future"), 1);
}
