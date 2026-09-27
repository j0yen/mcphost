//! PRD-mcphost-oauth-conformance-harness
//! AC5 (P0) — Given `mcphost oauth-probe --url http://127.0.0.1:<port>/mcp
//! --json` against the in-process host, When it runs, Then the JSON
//! matches the gate verdict table and the exit code is 0; Given one
//! scenario forced to `fail` via a test-only env, Then exit code 1.

use std::process::Command;

use crate::common;
use crate::oauthclient;
use common::TestServer;

// Both tests below spawn `mcphost oauth-probe` as a real OS subprocess and
// block synchronously on its output while the in-process `TestServer`'s
// axum task needs to keep running on the *same* tokio runtime to answer
// that subprocess's requests. A `current_thread` runtime (the `#[tokio::
// test]` default) has only one OS thread, so a synchronous `Command::
// output()` on it starves the server task entirely -- every request from
// the subprocess hangs until its own client-side timeout, which is exactly
// the "every scenario reads fail" failure this looked like before this was
// understood. `multi_thread` gives the blocking call its own worker thread
// so the server task keeps making progress concurrently.
#[tokio::test(flavor = "multi_thread")]
async fn probe_json_matches_gate_table_and_exit_code_is_zero() {
    let server = TestServer::start().await;
    let mcp_url = format!("{}/mcp", server.base_url);

    let http = reqwest::Client::new();
    let gate_results = oauthclient::run_all(&http, &mcp_url).await;

    let bin = env!("CARGO_BIN_EXE_mcphost");
    let output = Command::new(bin)
        .args(["oauth-probe", "--url", &mcp_url, "--json"])
        .output()
        .expect("spawn mcphost oauth-probe");

    assert!(
        output.status.success(),
        "exit code must be 0: status={:?} stderr={} stdout={}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );

    let probe_results: Vec<serde_json::Value> =
        serde_json::from_slice(&output.stdout).expect("--json output must parse as JSON");
    assert_eq!(probe_results.len(), gate_results.len(), "probe must cover every gate scenario");
    for gate in &gate_results {
        let probe = probe_results
            .iter()
            .find(|r| r["name"] == serde_json::json!(gate.name))
            .unwrap_or_else(|| panic!("probe output missing scenario {}", gate.name));
        assert_eq!(
            probe["verdict"],
            serde_json::to_value(gate.verdict).unwrap(),
            "scenario {} verdict must match the gate's own computed verdict",
            gate.name
        );
        assert_eq!(probe["family"], serde_json::json!(gate.family));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn force_fail_env_makes_exit_code_one() {
    let server = TestServer::start().await;
    let mcp_url = format!("{}/mcp", server.base_url);

    let gold = oauthclient::load_scenarios();
    let forced = gold.first().expect("at least one gold scenario").name.clone();

    let bin = env!("CARGO_BIN_EXE_mcphost");
    let output = Command::new(bin)
        .args(["oauth-probe", "--url", &mcp_url, "--json"])
        .env("MCPHOST_OAUTHCONF_FORCE_FAIL", &forced)
        .output()
        .expect("spawn mcphost oauth-probe");

    assert_eq!(
        output.status.code(),
        Some(1),
        "a forced fail must exit 1: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Not just "exit code 1 for any reason" -- exactly the forced scenario
    // reads fail and every other scenario keeps its real (pass/unsupported)
    // verdict, so this test can't pass vacuously the way it would if every
    // scenario happened to fail for an unrelated reason (e.g. the runtime-
    // starvation bug this file's doc comment above describes).
    let probe_results: Vec<serde_json::Value> =
        serde_json::from_slice(&output.stdout).expect("--json output must parse as JSON");
    for r in &probe_results {
        let name = r["name"].as_str().expect("scenario name");
        let verdict = r["verdict"].as_str().expect("scenario verdict");
        if name == forced {
            assert_eq!(verdict, "fail", "forced scenario {name} must read fail");
        } else {
            assert!(
                verdict == "pass" || verdict == "unsupported",
                "non-forced scenario {name} must keep its real verdict, got {verdict}"
            );
        }
    }
}
