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

// Hotfix outputs-field-generated-from-kind-registry: every spec field the
// served instructions name in backticks must be accepted by every kind the
// text claims it for, so the text can never promise a field a kind refuses
// (the `outputs`-on-echo disagreement).

fn backticked(text: &str) -> Vec<String> {
    text.split('`').skip(1).step_by(2).map(str::to_string).collect()
}

#[tokio::test]
async fn instructions_only_name_spec_fields_every_claimed_kind_accepts() {
    let kinds = mcphost::kinds::production_registry();
    let mut spec_fields: std::collections::BTreeSet<&str> = Default::default();
    for kind in kinds.all() {
        spec_fields.extend(kind.known_spec_fields().iter().copied());
    }
    spec_fields.extend(mcphost::kinds::UNIVERSAL_SPEC_FIELDS.iter().copied());
    let accepts = |kind: &str, field: &str| {
        mcphost::kinds::kinds_accepting_spec_field(&kinds, field).contains(&kind)
    };

    let server = common::TestServer::start_with_kinds(mcphost::kinds::production_registry()).await;
    let client = common::McpClient::new(&server.base_url);
    let init = client.initialize().await;
    let text = init["result"]["instructions"].as_str().expect("instructions");

    // The envelope-contract sentence claims its fields for EVERY kind.
    let start = text.find("Result envelope contract:").expect("envelope sentence");
    let end = text.find("host.tool_publish's `kind` argument").expect("kind sentence");
    let claimed: Vec<String> = backticked(&text[start..end])
        .into_iter()
        .filter(|t| spec_fields.contains(t.as_str()))
        .collect();
    assert!(claimed.iter().any(|f| f == "outputs"), "sentence must name outputs: {text}");
    for field in &claimed {
        for name in kinds.names() {
            assert!(accepts(name, field), "instructions claim `{field}` for every kind but {name} refuses it");
        }
    }

    // The kind-honoring sentence: fields listed before `<kind>` belong to it.
    let mut pending: Vec<String> = Vec::new();
    let mut checked = 0;
    for token in backticked(&text[end..]) {
        if kinds.names().contains(&token.as_str()) {
            for field in pending.drain(..) {
                assert!(accepts(&token, &field), "instructions tie `{field}` to {token}, which refuses it");
                checked += 1;
            }
        } else if spec_fields.contains(token.as_str()) {
            pending.push(token);
        }
    }
    assert!(checked >= 4, "expected source/python and upstream+method+url/http, checked {checked}");

    // And the registry itself: `outputs` is accepted by every kind.
    assert_eq!(
        mcphost::kinds::kinds_accepting_spec_field(&kinds, "outputs").len(),
        kinds.names().len()
    );
}
