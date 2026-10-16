//! mcphost-js-kind AC3 (P0) — Given the js runtime, When its `mcphost` object's keys are
//! listed, Then they equal `runner_script_registered_modules()` plus `call`
//! and `progress`.
//!
//! The keys are listed by a real tool running in the sandbox
//! (`Object.keys(mcphost)`), never read back from the Rust table the prelude
//! is generated from -- the python side is parsed back out of the python
//! runner's own script text, so the two bridges are compared as shipped.

use crate::common;
use mcphost::kinds::js::{self, CallParams};
use mcphost::kinds::python::runner_script_registered_modules;
use mcphost::sandbox::{self, NetworkMode, NoSidecar, SandboxOutcome};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

async fn run_tool(source: &str) -> Value {
    let dir = common::TempDataDir::new();
    let params = CallParams {
        source,
        timeout_s: 10,
        memory_mb: 256,
        network: NetworkMode::None,
        extra_env: Vec::new(),
        isolation: sandbox::detect_mechanism(),
    };
    let cancel = Arc::new(Mutex::new(None));
    let outcome = js::cold_call(&dir.0, &params, &json!({"args": {}}), &NoSidecar, &cancel)
        .await
        .expect("cold call");
    let SandboxOutcome::Exited { stdout, .. } = outcome else {
        panic!("tool did not exit cleanly: {outcome:?}");
    };
    serde_json::from_slice(&stdout).expect("envelope json")
}

#[tokio::test]
async fn mcphost_object_keys_equal_the_python_registered_modules_plus_call_and_progress() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envelope = run_tool("function main(args) { return Object.keys(mcphost); }\n").await;
    assert_eq!(envelope["ok"], json!(true), "{envelope}");
    let mut expected = runner_script_registered_modules();
    assert!(!expected.is_empty(), "the python runner registers modules");
    expected.push("call".to_string());
    expected.push("progress".to_string());
    // Equality as a set: the prelude's table is a sorted JSON object.
    let mut actual: Vec<String> = serde_json::from_value(envelope["result"].clone()).expect("array of keys");
    actual.sort();
    expected.sort();
    assert_eq!(actual, expected);
}

#[tokio::test]
async fn reading_a_member_that_is_not_a_bridge_names_the_available_ones() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envelope = run_tool("function main(args) { return mcphost.sqlite; }\n").await;
    assert_eq!(envelope["ok"], json!(false), "{envelope}");
    assert_eq!(envelope["error"], json!("ReferenceError"), "{envelope}");
    let message = envelope["message"].as_str().unwrap_or_default();
    assert!(message.starts_with("mcphost.sqlite is not a bridge; available: "), "{message}");
    for module in runner_script_registered_modules() {
        assert!(message.contains(&format!("mcphost.{module}")), "{module} missing from: {message}");
    }
}
