//! AC4 (PRD-mcphost-admin-signup-pause-tool) — Given calls with no arguments,
//! with both `message` and `resume: true`, with `resume: false` alone, with an
//! empty message, a message containing a newline, a message over 512 bytes,
//! and with `until` not in the future, When each is made by the admin, Then
//! each returns `invalid_params` naming the offending field and the pause
//! file's existence and content are unchanged.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer};
use serde_json::{Value, json};

fn cases() -> Vec<(&'static str, Value, &'static str)> {
    let past = mcphost::state::now_unix() - 60;
    vec![
        ("no arguments", json!({}), "message"),
        ("message and resume", json!({"message": "x", "resume": true}), "message"),
        ("resume false alone", json!({"resume": false}), "resume"),
        ("empty message", json!({"message": ""}), "message"),
        ("newline in message", json!({"message": "line one\nline two"}), "message"),
        ("carriage return in message", json!({"message": "line one\rline two"}), "message"),
        ("message over 512 bytes", json!({"message": "m".repeat(513)}), "message"),
        ("until in the past", json!({"message": "x", "until": past}), "until"),
        ("until zero", json!({"message": "x", "until": 0}), "until"),
        ("until with resume", json!({"resume": true, "until": mcphost::state::now_unix() + 600}), "until"),
    ]
}

async fn run_cases(server: &TestServer, admin: &McpClient) {
    let path = server.state.signup_pause.path().to_path_buf();
    for (label, args, field) in cases() {
        let before = std::fs::read_to_string(&path).ok();
        let err = admin
            .tools_call("admin.signup_pause", args)
            .await
            .expect_err(&format!("{label} must be refused"));
        // JSON-RPC -32602 for every case. The descriptor states `minLength: 1` and the
        // either-or `anyOf [message | resume]`, so an empty message and a call with
        // neither field are schema refusals (`args_invalid`) before the handler's own
        // `invalid_params` check can run; the `anyOf` refusal names the fields in `expected`.
        assert_eq!(err.code, -32602, "{label}: {err:?}");
        let schema_refused = label == "empty message" || label == "no arguments";
        let expected = if schema_refused { "args_invalid" } else { "invalid_params" };
        assert_eq!(err.error_code.as_deref(), Some(expected), "{label}: {err:?}");
        let named = err.message.contains(field) || err.data.get("expected").is_some_and(|e| e.to_string().contains(field));
        assert!(named, "{label}: refusal must name '{field}': {err:?}");
        assert_eq!(std::fs::read_to_string(&path).ok(), before, "{label}: pause file changed");
    }
}

#[tokio::test]
async fn aspt_ac04_invalid_calls_leave_a_missing_file_missing() {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    run_cases(&server, &admin).await;
    assert!(!server.state.signup_pause.path().exists());
}

#[tokio::test]
async fn aspt_ac04_invalid_calls_leave_an_existing_file_intact() {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    std::fs::write(server.state.signup_pause.path(), "keep me\nuntil=9999999999\n").expect("seed file");
    run_cases(&server, &admin).await;
    let after = std::fs::read_to_string(server.state.signup_pause.path()).expect("file still there");
    assert_eq!(after, "keep me\nuntil=9999999999\n");
}
