//! PRD-mcphost-org-export-and-offboard-v2: fixture helpers for the `orgx_ac*` tests.

use std::collections::HashMap;
use std::io::Read;
use std::time::{Duration, Instant};

use flate2::read::GzDecoder;
use serde_json::{Value, json};
use tar::Archive;

use super::{McpClient, TestServer, extract_structured, signup};

/// An org built by invite: the owner's client plus one client per member.
pub struct OrgFixture {
    pub org: String,
    pub owner_ns: String,
    pub owner: McpClient,
    pub members: Vec<(String, McpClient)>,
}

impl OrgFixture {
    /// Every namespace of the org, owner first.
    pub fn namespaces(&self) -> Vec<String> {
        std::iter::once(self.owner_ns.clone())
            .chain(self.members.iter().map(|(ns, _)| ns.clone()))
            .collect()
    }
}

/// Publishes `n` echo tools named `<prefix>_<i>` for `client`, each with its own spec.
pub async fn publish_tools(client: &McpClient, prefix: &str, n: usize) {
    for i in 0..n {
        let name = format!("{prefix}_{i}");
        client
            .tools_call(
                "host.tool_publish",
                json!({"name": name, "kind": "echo", "spec": {"schema": {"type": "object", "title": name}}}),
            )
            .await
            .unwrap_or_else(|e| panic!("publish {prefix}_{i} failed: {} {}", e.code, e.message));
    }
}

/// An owner and `members` joined tenants (each joins with its own key), each
/// holding `tools_each` tools named `<ns-prefix>_<i>`.
pub async fn fixture_org(server: &TestServer, org: &str, members: usize, tools_each: usize) -> OrgFixture {
    let (owner_ns, owner_key) = signup(&server.base_url, "Org Owner").await;
    let owner = McpClient::with_bearer(&server.base_url, &owner_key);
    let created = extract_structured(&owner.tools_call("host.org.create", json!({"name": org})).await.expect("create"));
    let code = created["invite_url"]
        .as_str()
        .expect("invite_url")
        .trim_start_matches(&server.base_url)
        .trim_start_matches("/o/")
        .trim_end_matches("/mcp")
        .to_string();
    publish_tools(&owner, "own", tools_each).await;
    let mut joined = Vec::new();
    for i in 0..members {
        let (ns, key) = signup(&server.base_url, &format!("Member {i}")).await;
        let client = McpClient::with_bearer(&server.base_url, &key);
        client.tools_call("host.org.join", json!({"code": code})).await.expect("join");
        publish_tools(&client, &format!("m{i}"), tools_each).await;
        joined.push((ns, client));
    }
    OrgFixture { org: org.to_string(), owner_ns, owner, members: joined }
}

/// Rotates the member's key and drops the new value, so the test holds no
/// member credential afterwards.
pub async fn rotate_and_discard(client: &McpClient) {
    client.tools_call("host.key_rotate", json!({})).await.expect("rotate");
}

/// Polls `host.runs.get` until the run is `done` or `error`.
pub async fn wait_done(client: &McpClient, run_id: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let got = extract_structured(&client.tools_call("host.runs.get", json!({"run_id": run_id})).await.expect("runs.get"));
        if got["status"] == json!("done") || got["status"] == json!("error") {
            return got;
        }
        assert!(Instant::now() < deadline, "export never finished: {got:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Runs `host.org.export` as `client` and returns the finished run's `result`.
pub async fn run_org_export(client: &McpClient) -> Value {
    let started = extract_structured(&client.tools_call("host.org.export", json!({})).await.expect("org export"));
    let run_id = started["run_id"].as_str().expect("run_id").to_string();
    let finished = wait_done(client, &run_id).await;
    assert_eq!(finished["status"], json!("done"), "org export failed: {finished:?}");
    finished["result"].clone()
}

/// Every `path -> bytes` entry of a `.tar.gz`.
pub fn read_tar_gz(bytes: &[u8]) -> HashMap<String, Vec<u8>> {
    let mut archive = Archive::new(GzDecoder::new(bytes));
    let mut out = HashMap::new();
    for entry in archive.entries().expect("tar entries") {
        let mut entry = entry.expect("tar entry");
        let path = entry.path().expect("entry path").to_string_lossy().to_string();
        let mut data = Vec::new();
        entry.read_to_end(&mut data).expect("read entry");
        out.insert(path, data);
    }
    out
}

/// GETs a result's signed `download_url` and returns the bundle's entries.
pub async fn download_bundle(result: &Value) -> HashMap<String, Vec<u8>> {
    let url = result["download_url"].as_str().expect("download_url");
    let resp = reqwest::get(url).await.expect("download");
    assert_eq!(resp.status(), 200, "signed url must serve the bundle");
    read_tar_gz(&resp.bytes().await.expect("body"))
}
