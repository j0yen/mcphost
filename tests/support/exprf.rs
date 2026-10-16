//! Shared fixture for the `exprf_ac*` tests
//! (PRD-mcphost-example-proof-nightly-and-hide): a `TestServer`, an admin
//! client, a tenant client, and a stub proof directory whose scripts exit
//! 0 or 1 by example name, so the runner never touches a real proof.

#![allow(dead_code)]

use std::path::PathBuf;

use crate::common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

pub struct Fixture {
    pub server: TestServer,
    pub admin: McpClient,
    pub tenant: McpClient,
}

pub async fn start() -> Fixture {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let (_ns, key) = signup(&server.base_url, "exprf tenant").await;
    let tenant = McpClient::with_bearer(&server.base_url, &key);
    Fixture { server, admin, tenant }
}

/// Every embedded example name, in index order.
pub fn names() -> Vec<&'static str> {
    mcphost::examples::EXAMPLES.iter().map(|e| e.name).collect()
}

/// A temp dir holding `<name>/proof.sh` for each name: `exit 1` for those in
/// `failing`, `exit 0` otherwise. A `marker` file is touched by every script
/// that runs, so a test can prove nothing executed.
pub fn stub_proofs(failing: &[&str], marker: Option<&std::path::Path>) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("exprf-stub-{}-{}", std::process::id(), mcphost::state::now_unix_ms()));
    for ex in mcphost::examples::EXAMPLES {
        let name = ex.name;
        let d = dir.join(name).join(std::path::Path::new(ex.proof).parent().unwrap_or(std::path::Path::new("")));
        std::fs::create_dir_all(&d).expect("stub dir");
        let touch = marker.map(|m| format!("touch '{}'\n", m.display())).unwrap_or_default();
        let code = i32::from(failing.contains(&name));
        std::fs::write(d.join(std::path::Path::new(ex.proof).file_name().expect("proof file name")), format!("#!/usr/bin/env bash\n{touch}echo \"stub proof {name}\"\nexit {code}\n")).expect("stub proof");
    }
    dir
}

pub async fn report(admin: &McpClient, name: &str, ok: bool) -> Result<Value, crate::common::RpcError> {
    admin
        .tools_call(
            "admin.example_proof_report",
            json!({"name": name, "ok": ok, "duration_s": 1.5, "log_excerpt": format!("line for {name}"), "runner": "orch"}),
        )
        .await
        .map(|r| extract_structured(&r))
}

pub async fn listed_names(client: &McpClient) -> Vec<String> {
    let listed = extract_structured(&client.tools_call("host.example.list", json!({})).await.expect("host.example.list"));
    listed["examples"].as_array().expect("examples").iter().map(|e| e["name"].as_str().unwrap_or_default().to_string()).collect()
}
