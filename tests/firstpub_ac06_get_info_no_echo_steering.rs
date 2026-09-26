//! PRD-mcphost-first-publish-real-kind
//! AC6 (P0) -- Given `host.get_info`, When read on any plan, Then the
//! response contains no text steering to echo (outside the tools list).
//!
//! Covers both the ordinary-startup instructions text and the
//! sandbox-unready NOTE (PRD-mcphost-sandbox-ready P1 requirement 7), which
//! used to append "publish echo or http instead."

use crate::common;
use common::{fake_interpreter_failing, python_kind_registry_with_selftest};

fn assert_instructions_never_steer_to_echo(instructions: &str) {
    assert!(
        !instructions.to_lowercase().contains("echo"),
        "host.get_info instructions must never mention echo: {instructions}"
    );
}

#[tokio::test]
async fn get_info_never_mentions_echo_when_sandbox_is_ready() {
    let server = common::TestServer::start().await;
    let client = common::McpClient::new(&server.base_url);

    let init = client.initialize().await;
    let instructions = init["result"]["instructions"]
        .as_str()
        .expect("initialize result carries instructions");
    assert_instructions_never_steer_to_echo(instructions);
}

#[tokio::test]
async fn get_info_never_mentions_echo_when_sandbox_is_unready() {
    let data_dir = common::TempDataDir::new();
    let (kinds, py) = python_kind_registry_with_selftest(&data_dir.0, 300);
    py.set_selftest_interpreter_for_test(Some(&fake_interpreter_failing(
        "bwrap: loopback: Failed RTM_NEWADDR: Operation not permitted",
    )));
    py.run_startup_selftest().await;

    let server = common::TestServer::start_with_kinds(kinds).await;
    let client = common::McpClient::new(&server.base_url);

    let init = client.initialize().await;
    let instructions = init["result"]["instructions"]
        .as_str()
        .expect("initialize result carries instructions");
    // The NOTE must still fire (unchanged PRD-mcphost-sandbox-ready
    // behavior) -- just without steering to echo.
    assert!(
        instructions.contains("sandbox_unavailable"),
        "the unready-sandbox NOTE must still be present: {instructions}"
    );
    assert_instructions_never_steer_to_echo(instructions);
}
