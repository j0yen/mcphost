//! AC12 — Given a running server, When `synthorg consume --preflight <url>`
//! runs, Then it exits 0.
//!
//! Two halves:
//!
//! 1. `protocol_half_matches_synthorg_preflight` — always runs, in-process,
//!    like every other `tests/ac*.rs`. It exercises exactly the two
//!    requests `synthorg consume --preflight` makes (source:
//!    `synthorg/src/synthorg/consume.py` `run_preflight`, ~line 1203): a
//!    raw `initialize` POST with `Accept: application/json, text/event-stream`
//!    (asserting HTTP < 400 and a non-empty `MCP-Protocol-Version` response
//!    header), then `initialize` + `tools/list` through a client session
//!    (asserting a `signup` tool is listed).
//! 2. `synthorg_binary_preflight_exits_zero_when_available` — if a real
//!    `synthorg` is invokable (bare binary on PATH, else
//!    `uv run --project <repos/synthorg> synthorg` as a fallback), spawns
//!    the real `mcphost` binary (`ac11_load_smoke.rs`'s pattern) and runs
//!    `synthorg consume --preflight` against it end to end, asserting exit
//!    0 and that stdout contains "ok". If neither invocation works, this
//!    half prints "skipped: synthorg not on PATH" and returns without
//!    asserting further -- the in-process half above always runs and
//!    always asserts regardless, so this file is never `#[ignore]`d.

use crate::common;
use common::{McpClient, TestServer};
use serde_json::json;
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    listener.local_addr().unwrap().port()
}

#[tokio::test]
async fn protocol_half_matches_synthorg_preflight() {
    let server = TestServer::start().await;
    let http = reqwest::Client::new();

    // (a) raw `initialize` POST -- what `run_preflight`'s first request
    // checks: HTTP status and the `MCP-Protocol-Version` response header.
    let resp = http
        .post(format!("{}/mcp", server.base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2026-07-28",
                "capabilities": {},
                "clientInfo": {"name": "synthorg-consume-preflight", "version": "0.1"}
            }
        }))
        .send()
        .await
        .expect("send initialize");
    assert!(
        resp.status().as_u16() < 400,
        "initialize must not error: {}",
        resp.status()
    );
    let protocol_version = resp
        .headers()
        .get("MCP-Protocol-Version")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        !protocol_version.is_empty(),
        "initialize response must carry a non-empty MCP-Protocol-Version header"
    );

    // (b) `initialize` + `tools/list` through a client session -- what
    // `run_preflight`'s second half checks, over the real client path
    // (`streamable_http_client` + `ClientSession`), not a raw POST.
    let client = McpClient::new(&server.base_url);
    let init = client.initialize().await;
    assert!(
        init.get("error").is_none(),
        "client-session initialize should not error: {init:?}"
    );
    let tools = client
        .tools_list()
        .await
        .expect("tools/list should succeed unauthenticated");
    let tool_names: Vec<&str> = tools["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(
        tool_names.contains(&"signup"),
        "tools/list must list a 'signup' tool, got {tool_names:?}"
    );
}

/// The absolute path this environment's `synthorg` checkout lives at (per
/// this build tick's instructions). Only used as a fallback invocation
/// (`uv run --project <path> synthorg`) when the bare `synthorg` binary is
/// not on PATH.
const SYNTHORG_PROJECT: &str = "/home/jsy/repos/synthorg";

/// The argv prefix to invoke `synthorg` with, preferring the bare binary
/// and falling back to `uv run --project`. `None` if neither works here.
fn synthorg_invocation() -> Option<Vec<String>> {
    if Command::new("synthorg")
        .arg("--help")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
    {
        return Some(vec!["synthorg".to_string()]);
    }
    if std::path::Path::new(SYNTHORG_PROJECT).exists()
        && Command::new("uv")
            .args(["run", "--project", SYNTHORG_PROJECT, "synthorg", "--help"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    {
        return Some(vec![
            "uv".to_string(),
            "run".to_string(),
            "--project".to_string(),
            SYNTHORG_PROJECT.to_string(),
            "synthorg".to_string(),
        ]);
    }
    None
}

#[tokio::test]
async fn synthorg_binary_preflight_exits_zero_when_available() {
    let Some(invocation) = synthorg_invocation() else {
        println!(
            "skipped: synthorg not on PATH (bare binary and uv run --project fallback both failed)"
        );
        return;
    };

    // Spawn the real `mcphost` binary as a subprocess, per `ac11_load_smoke.rs`'s
    // pattern, so this exercises the actual CLI/HTTP stack synthorg would
    // see in production, not just the in-process test harness.
    let data_dir = std::env::temp_dir().join(format!("mcphost-ac12-{}", std::process::id()));
    std::fs::create_dir_all(&data_dir).unwrap();
    let port = free_port();
    let bin = env!("CARGO_BIN_EXE_mcphost");
    let mut child = Command::new(bin)
        .arg("serve")
        .env("MCPHOST_DATA_DIR", &data_dir)
        .env("MCPHOST_BIND", format!("127.0.0.1:{port}"))
        .env("MCPHOST_ADMIN_KEY", "ac12-admin-key")
        .env("MCPHOST_SECRET_KEY", "ac12-secret-key")
        .env("MCPHOST_LOG_LEVEL", "warn")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn mcphost serve");
    let stderr = child.stderr.take();
    let _child = ChildGuard(child);

    let base_url = format!("http://127.0.0.1:{port}");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if reqwest::get(format!("{base_url}/healthz")).await.is_ok() {
            break;
        }
        if Instant::now() > deadline {
            let mut buf = String::new();
            if let Some(mut s) = stderr {
                let _ = s.read_to_string(&mut buf);
            }
            panic!("mcphost serve did not become healthy in time; stderr:\n{buf}");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let mcp_url = format!("{base_url}/mcp");
    let mut cmd = Command::new(&invocation[0]);
    cmd.args(&invocation[1..]);
    cmd.args(["consume", "--preflight", "--endpoint", &mcp_url]);
    let output = cmd.output().expect("run synthorg consume --preflight");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "synthorg consume --preflight must exit 0; status={:?} stdout={stdout} stderr={stderr}",
        output.status
    );
    assert!(
        stdout.contains("ok"),
        "synthorg consume --preflight stdout should contain 'ok': {stdout}"
    );

    let _ = std::fs::remove_dir_all(&data_dir);
}
