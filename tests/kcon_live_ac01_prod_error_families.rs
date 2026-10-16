//! PRD-mcphost-kind-contract-error-families-live -- the shared error contract
//! (`src/kinds/contract.rs` families, `src/doctor.rs` `data.line` /
//! `data.source_line`) proven end to end through the MCP surface.
//!
//! AC1: in-process `TestServer` leg (always on). AC2: the live leg is gated
//! on `MCPHOST_LIVE=1` and skips with a printed hint otherwise. AC3 (Live)
//! is `MCPHOST_LIVE=1 MCPHOST_URL=https://mcphost.dev cargo test --test
//! kcon_live_ac01_prod_error_families`.

use std::time::Duration;

use base64::Engine as _;
use serde_json::{Value, json};

use crate::common::{self, McpClient, RpcError, TestServer, publish, signup};

const SKIP_HINT: &str = "skipping live error-families leg: set MCPHOST_LIVE=1 (and optionally \
                         MCPHOST_URL) to run it against a real endpoint";

/// Pure predicate, so the gating is asserted without touching the ambient env.
fn live_mode_enabled(raw: Option<&str>) -> bool {
    raw == Some("1")
}

fn python_spec() -> Value {
    // The raise is on line 2 of the tool source.
    json!({"source": "def main(args):\n    raise ValueError('boom')\n"})
}

fn wasm_spec() -> Value {
    let bytes = include_bytes!("fixtures/wasm/trap.wasm");
    json!({"component": base64::engine::general_purpose::STANDARD.encode(bytes)})
}

/// Publishes both tools as `client`'s tenant, calls them, and asserts the
/// four contract clauses, each message printing the whole error `data`.
async fn assert_error_families(client: &McpClient) {
    let py = publish(client, "kcon_live_py", "python", python_spec()).await;
    let wasm = publish(client, "kcon_live_wasm", "wasm", wasm_spec()).await;

    let py_err = call_error(client, &py).await;
    assert_eq!(
        py_err.data["families"],
        json!(["tool_code"]),
        "python error families: {}",
        py_err.data
    );
    assert_eq!(py_err.data["line"], json!(2), "python error line: {}", py_err.data);
    assert_eq!(
        py_err.data["source_line"].as_str(),
        Some("raise ValueError('boom')"),
        "python error source_line: {}",
        py_err.data
    );

    let wasm_err = call_error(client, &wasm).await;
    assert_eq!(
        wasm_err.data["families"],
        json!(["tool_code"]),
        "wasm error families: {}",
        wasm_err.data
    );
    assert!(
        wasm_err.data.as_object().is_some_and(|o| !o.contains_key("source_line")),
        "a wasm trap has no source_line key: {}",
        wasm_err.data
    );
}

async fn call_error(client: &McpClient, name: &str) -> RpcError {
    common::poll_until_ready(client, name, json!({}), Duration::from_secs(60))
        .await
        .expect_err("the tool must fail with a structured error")
}

#[tokio::test]
async fn mcphost_kind_contract_error_families_live_ac01_in_process_error_families() {
    if mcphost::sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", mcphost::sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let data_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(common::five_kinds_registry(&data_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "kcon-live-in-process").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    assert_error_families(&client).await;
}

#[tokio::test]
async fn mcphost_kind_contract_error_families_live_ac02_live_leg_skips_without_flag() {
    assert!(!live_mode_enabled(None), "unset must disable live mode");
    assert!(!live_mode_enabled(Some("0")), "0 must disable live mode");
    assert!(!live_mode_enabled(Some("")), "empty must disable live mode");
    assert!(live_mode_enabled(Some("1")), "1 enables live mode");
    assert!(SKIP_HINT.contains("MCPHOST_LIVE=1"));
}

#[tokio::test]
async fn mcphost_kind_contract_error_families_live_ac03_live_leg_when_enabled() {
    live_leg().await;
}

/// AC3 (Live): signs up as a `kcon-live-` tenant on `$MCPHOST_URL`.
async fn live_leg() {
    if !live_mode_enabled(std::env::var("MCPHOST_LIVE").ok().as_deref()) {
        eprintln!("{SKIP_HINT}");
        return;
    }
    let base_url = std::env::var("MCPHOST_URL").unwrap_or_else(|_| "https://mcphost.dev".into());
    let base_url = base_url.trim_end_matches("/mcp").to_string();
    let (_ns, key) = signup(&base_url, "kcon-live-error-families").await;
    let client = McpClient::with_bearer(&base_url, &key);
    assert_error_families(&client).await;
}
