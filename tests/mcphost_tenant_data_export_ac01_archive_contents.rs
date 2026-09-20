//! PRD-mcphost-tenant-data-export
//! AC1 (P0) -- Given a tenant with two tools, three state keys, and one
//! secret, When `host.export` runs to completion, Then the archive
//! contains both tool sources, three state files, `secrets.txt` with one
//! name and no value, and `manifest.json`.

use std::collections::HashMap;
use std::io::Read;
use std::time::{Duration, Instant};

use flate2::read::GzDecoder;
use tar::Archive;

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

fn read_archive_entries(path: &std::path::Path) -> HashMap<String, Vec<u8>> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read archive {path:?}: {e}"));
    let decoder = GzDecoder::new(&bytes[..]);
    let mut archive = Archive::new(decoder);
    let mut out = HashMap::new();
    for entry in archive.entries().expect("tar entries") {
        let mut entry = entry.expect("tar entry");
        let entry_path = entry.path().expect("entry path").to_string_lossy().to_string();
        let mut data = Vec::new();
        entry.read_to_end(&mut data).expect("read entry data");
        out.insert(entry_path, data);
    }
    out
}

async fn wait_for_done(client: &McpClient, run_id: &str) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(10);
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

#[tokio::test]
async fn export_archive_contains_tools_state_secrets_and_manifest() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Export AC1 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    for name in ["alpha", "beta"] {
        client
            .tools_call(
                "host.tool_publish",
                json!({"name": name, "kind": "echo", "spec": {"schema": {"type": "object"}}}),
            )
            .await
            .unwrap_or_else(|e| panic!("publish {name} failed: {} {}", e.code, e.message));
    }

    for (key_name, value) in [("k1", 1), ("k2", 2), ("k3", 3)] {
        client
            .tools_call("host.state.set", json!({"key": key_name, "value": value}))
            .await
            .unwrap_or_else(|e| panic!("state.set {key_name} failed: {} {}", e.code, e.message));
    }

    client
        .tools_call("host.secret_set", json!({"name": "api_key", "value": "sekrit"}))
        .await
        .expect("secret_set ok");

    let enqueue = extract_structured(
        &client
            .tools_call("host.export", json!({}))
            .await
            .expect("host.export ok"),
    );
    let run_id = enqueue["run_id"].as_str().expect("run_id field").to_string();

    let finished = wait_for_done(&client, &run_id).await;
    assert_eq!(finished["status"], json!("done"), "export failed: {finished:?}");
    let result = &finished["result"];
    assert!(result["download_url"].as_str().is_some(), "result: {result:?}");
    let manifest = result["manifest"].as_array().expect("manifest array");
    assert_eq!(manifest.len(), 2, "manifest: {manifest:?}");

    let archive_path = server
        .state
        .db
        .data_dir()
        .join("exports")
        .join(format!("{run_id}.tar.gz"));
    let entries = read_archive_entries(&archive_path);

    assert!(entries.contains_key("manifest.json"), "entries: {:?}", entries.keys());
    assert!(
        entries.contains_key("tools/alpha/v1/config.json"),
        "entries: {:?}",
        entries.keys()
    );
    assert!(
        entries.contains_key("tools/beta/v1/config.json"),
        "entries: {:?}",
        entries.keys()
    );
    assert!(entries.contains_key("state/k1.json"), "entries: {:?}", entries.keys());
    assert!(entries.contains_key("state/k2.json"), "entries: {:?}", entries.keys());
    assert!(entries.contains_key("state/k3.json"), "entries: {:?}", entries.keys());

    let secrets_txt = String::from_utf8(entries["secrets.txt"].clone()).expect("secrets.txt utf8");
    assert_eq!(secrets_txt.trim(), "api_key", "secrets.txt must name only, never a value");
    assert!(!secrets_txt.contains("sekrit"));
}
