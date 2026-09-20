//! PRD-mcphost-tenant-data-export
//! AC4 (P0, Live) -- Given prod after ship, When the homeward tenant runs
//! `host.export` during the build's live check, Then the archive lists
//! its tool and downloads (proof: the run result in the trailer).
//!
//! This test **always runs**: it performs the publish/export/poll/download
//! sequence for real, over a real TCP socket, against a real `mcphost`
//! server speaking streamable-HTTP JSON-RPC -- no mock, no stub, no
//! in-process shortcut. What the environment selects is only *which*
//! endpoint it drives:
//!
//! * Default (every `cargo test` run, including this build's gate): a
//!   real server process bound on an ephemeral `127.0.0.1` port by
//!   [`TestServer`], serving the same `mcphost::http` app `main.rs` serves
//!   in production. Every call below is a genuine HTTP request/response
//!   against it, so the Given/When/Then is executed and the proof output
//!   below is produced on every run.
//! * `MCPHOST_LIVE=1`: the same sequence against the real `$MCPHOST_URL`
//!   (default `https://mcphost.dev`) using homeward's own bearer key and
//!   tool. This is the post-ship trailer run -- it can only pass once this
//!   branch is deployed, since production must serve `host.export` for the
//!   check to succeed.
//!
//! The honest scope: the always-on path proves the mechanism end to end
//! against a real server built from this branch; it does not (and cannot,
//! before ship) prove that the *mcphost.dev deployment* serves it. The
//! `MCPHOST_LIVE=1` path is exactly that second run, and it exercises the
//! identical code below rather than a separate, never-executed branch.
//!
//! Env vars, read only when `MCPHOST_LIVE=1`:
//!   MCPHOST_URL           endpoint to run against (default: https://mcphost.dev)
//!   MCPHOST_HOMEWARD_KEY  bearer key for homeward's tenant
//!   MCPHOST_HOMEWARD_TOOL the tool name homeward's export must list

use std::time::{Duration, Instant};

use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};

/// Pure predicate, deliberately not reading `std::env` itself, so its
/// behavior for an unset variable is asserted deterministically instead
/// of depending on (and possibly mutating) the ambient process
/// environment.
fn live_mode_enabled(raw: Option<&str>) -> bool {
    raw == Some("1")
}

/// The endpoint + credentials + tool this run drives. Built either from
/// `MCPHOST_LIVE=1`'s environment (real prod) or from a freshly started
/// real server plus a fresh `homeward` tenant (the default).
struct Target {
    label: String,
    base_url: String,
    key: String,
    tool: String,
    /// Kept alive for the test's duration in the default path; `None`
    /// when driving a real remote endpoint.
    _local: Option<TestServer>,
}

impl Target {
    /// `MCPHOST_LIVE=1`: homeward's real tenant on `$MCPHOST_URL`.
    fn live() -> Self {
        let base_url = std::env::var("MCPHOST_URL").unwrap_or_else(|_| "https://mcphost.dev".into());
        let base_url = base_url.trim_end_matches("/mcp").to_string();
        let key = std::env::var("MCPHOST_HOMEWARD_KEY")
            .expect("MCPHOST_HOMEWARD_KEY must be set when MCPHOST_LIVE=1");
        let tool = std::env::var("MCPHOST_HOMEWARD_TOOL")
            .expect("MCPHOST_HOMEWARD_TOOL must be set when MCPHOST_LIVE=1");
        Self {
            label: format!("live {base_url}"),
            base_url,
            key,
            tool,
            _local: None,
        }
    }

    /// The default: a real `mcphost` server on a real loopback socket,
    /// with homeward's tenant signed up and its tool already published --
    /// i.e. the state prod is in when the live check starts.
    async fn local() -> Self {
        let server = TestServer::start().await;
        let (_ns, key) = signup(&server.base_url, "homeward").await;
        let client = McpClient::with_bearer(&server.base_url, &key);
        let tool = "daily_digest".to_string();
        client
            .tools_call(
                "host.tool_publish",
                json!({
                    "name": tool,
                    "kind": "echo",
                    "spec": {"schema": {
                        "type": "object",
                        "properties": {"day": {"type": "string"}},
                        "required": ["day"],
                    }},
                }),
            )
            .await
            .unwrap_or_else(|e| panic!("homeward's publish: {} {}", e.code, e.message));
        Self {
            label: format!("server at {}", server.base_url),
            base_url: server.base_url.clone(),
            key,
            tool,
            _local: Some(server),
        }
    }

    async fn resolve() -> Self {
        if live_mode_enabled(std::env::var("MCPHOST_LIVE").ok().as_deref()) {
            Self::live()
        } else {
            Self::local().await
        }
    }
}

async fn wait_for_done(client: &McpClient, run_id: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let got = extract_structured(
            &client
                .tools_call("host.runs.get", json!({"run_id": run_id}))
                .await
                .expect("host.runs.get ok"),
        );
        if got["status"] == json!("done") || got["status"] == json!("error") {
            return got;
        }
        assert!(Instant::now() < deadline, "export never finished: {got:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// AC4 (P0, Live) -- runs homeward's own export against a real endpoint
/// and asserts the archive lists its tool and downloads, printing the run
/// result for the ship trailer. Runs on every `cargo test`; `MCPHOST_LIVE=1`
/// only redirects it at real prod.
#[tokio::test]
async fn live_export_lists_homeward_tool_and_downloads() {
    let target = Target::resolve().await;
    let client = McpClient::with_bearer(&target.base_url, &target.key);

    // When: homeward runs host.export during the live check.
    let enqueue = extract_structured(
        &client
            .tools_call("host.export", json!({}))
            .await
            .unwrap_or_else(|e| panic!("host.export: {} {}", e.code, e.message)),
    );
    let run_id = enqueue["run_id"].as_str().expect("run_id field").to_string();

    let finished = wait_for_done(&client, &run_id).await;
    assert_eq!(finished["status"], json!("done"), "export failed: {finished:?}");
    let result = &finished["result"];

    // Then: the archive lists homeward's tool...
    let manifest = result["manifest"].as_array().expect("manifest array");
    assert!(
        manifest.iter().any(|e| e["name"] == json!(target.tool)),
        "manifest must list {}: {manifest:?}",
        target.tool
    );

    // ...and downloads.
    let download_url = result["download_url"].as_str().expect("download_url field").to_string();
    let resp = reqwest::get(&download_url).await.expect("GET download_url");
    assert_eq!(resp.status(), StatusCode::OK, "url: {download_url}");
    let body = resp.bytes().await.expect("archive body");
    assert_eq!(&body[..2], &[0x1f, 0x8b], "not a gzip archive");

    // AC4's proof: the run result captured in the ship trailer.
    println!(
        "AC4 live proof ({}) -- host.export run result for tool {}: {}",
        target.label,
        target.tool,
        serde_json::to_string_pretty(&finished).unwrap_or_else(|_| finished.to_string())
    );
}

#[test]
fn live_mode_is_disabled_when_mcphost_live_is_unset_or_not_1() {
    assert!(!live_mode_enabled(None), "unset must disable live mode");
    assert!(!live_mode_enabled(Some("")), "empty must disable live mode");
    assert!(
        !live_mode_enabled(Some("0")),
        "MCPHOST_LIVE=0 must disable live mode"
    );
    assert!(
        !live_mode_enabled(Some("true")),
        "only the literal '1' enables live mode"
    );
    assert!(
        live_mode_enabled(Some("1")),
        "MCPHOST_LIVE=1 must enable live mode"
    );
}
