//! PRD-mcphost-oauth-first-entry-url
//! AC6 (P0) — Given the router, the metadata route, `www/` client pages,
//! `llms-install.md`, and `host.quickstart`, When the `oauthentry`
//! path-contract test runs, Then every occurrence of the entry path equals
//! `OAUTH_ENTRY_PATH` and `sandbox-api-doc-check.sh` passes.

use crate::common;
use common::TestServer;
use mcphost::oauth::{OAUTH_ENTRY_PATH, entry_metadata_path, entry_resource_uri};
use serde_json::json;
use std::path::PathBuf;

fn repo_file(rel: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Every client page that names the entry URL.
const CLIENT_DOCS: &[&str] = &[
    "README.md",
    "llms-install.md",
    "www/llms-install.md",
    "www/llms.txt",
    "www/llms-full.txt",
    "www/index.html",
    "docs/oauth-connect.md",
];

/// Every `https://mcphost.dev/login/...` token in `text`.
fn entry_urls(text: &str) -> Vec<String> {
    let needle = "https://mcphost.dev/login/";
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find(needle) {
        let tail = &rest[at..];
        let end = tail
            .find(|c: char| !(c.is_ascii_alphanumeric() || "/._-:".contains(c)))
            .unwrap_or(tail.len());
        out.push(tail[..end].to_string());
        rest = &tail[end..];
    }
    out
}

#[test]
fn every_client_page_names_exactly_the_constant_entry_url() {
    let want = format!("https://mcphost.dev{OAUTH_ENTRY_PATH}");
    for doc in CLIENT_DOCS {
        let found = entry_urls(&repo_file(doc));
        assert!(!found.is_empty(), "{doc} must name the entry URL {want}");
        for url in found {
            assert_eq!(url, want, "{doc} names an entry path other than OAUTH_ENTRY_PATH");
        }
    }
}

#[test]
fn no_source_file_spells_the_entry_path_as_a_literal_outside_the_constant() {
    let mut offenders = Vec::new();
    for entry in std::fs::read_dir(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")).expect("read src") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") || path.file_name().and_then(|n| n.to_str()) == Some("oauth.rs") {
            continue;
        }
        for (n, line) in std::fs::read_to_string(&path).expect("read source").lines().enumerate() {
            let code = line.trim_start();
            if !code.starts_with("//") && code.contains(OAUTH_ENTRY_PATH) {
                offenders.push(format!("{}:{}", path.display(), n + 1));
            }
        }
    }
    assert!(offenders.is_empty(), "the router, metadata, and install table must read OAUTH_ENTRY_PATH: {offenders:?}");
}

#[tokio::test]
async fn router_metadata_and_quickstart_all_use_the_one_constant() {
    let server = TestServer::start().await;
    let http = reqwest::Client::new();

    let entry = http
        .post(format!("{}{OAUTH_ENTRY_PATH}", server.base_url))
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}))
        .send()
        .await
        .expect("POST entry");
    assert_eq!(entry.status(), reqwest::StatusCode::UNAUTHORIZED);
    let challenge = entry.headers().get(reqwest::header::WWW_AUTHENTICATE).expect("challenge").to_str().unwrap().to_string();
    assert!(challenge.contains(&format!("{}{}", server.base_url, entry_metadata_path())), "{challenge}");

    let doc: serde_json::Value = http
        .get(format!("{}{}", server.base_url, entry_metadata_path()))
        .send()
        .await
        .expect("GET document")
        .json()
        .await
        .expect("document json");
    assert_eq!(doc["resource"], json!(entry_resource_uri(&server.base_url)));

    let anonymous = common::extract_structured(
        &common::McpClient::new(&server.base_url).tools_call("host.quickstart", json!({})).await.expect("anonymous quickstart"),
    );
    assert_eq!(anonymous["oauth_entry_url"], json!(entry_resource_uri(&server.base_url)), "{anonymous}");

    let (_ns, key) = common::signup(&server.base_url, "Quickstart Caller").await;
    let tenant = common::extract_structured(
        &common::McpClient::with_bearer(&server.base_url, &key)
            .tools_call("host.quickstart", json!({"kind": "echo"}))
            .await
            .expect("tenant quickstart"),
    );
    assert_eq!(tenant["oauth_entry_url"], json!(entry_resource_uri(&server.base_url)), "{tenant}");
}

#[test]
fn the_install_table_gives_claude_ai_and_chatgpt_the_entry_url_and_a_personal_base_keeps_its_own() {
    let public = mcphost::install_links::for_url("https://mcphost.dev");
    let entry = entry_resource_uri("https://mcphost.dev");
    for id in ["claude_ai", "chatgpt"] {
        let surface = public.iter().find(|s| s.id == id).expect("surface");
        assert!(surface.artefact.contains(&entry), "{id}: {}", surface.artefact);
    }
    let personal = mcphost::install_links::for_url("https://mcphost.dev/u/abc123");
    for id in ["claude_ai", "chatgpt"] {
        let surface = personal.iter().find(|s| s.id == id).expect("surface");
        assert!(surface.artefact.contains("https://mcphost.dev/u/abc123/mcp"), "{id}: {}", surface.artefact);
        assert!(!surface.artefact.contains(OAUTH_ENTRY_PATH), "{id}: {}", surface.artefact);
    }
}

#[test]
fn sandbox_api_doc_check_passes() {
    let _doc_files = common::DOC_CHECK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = std::process::Command::new("bash")
        .arg("scripts/sandbox-api-doc-check.sh")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run sandbox-api-doc-check.sh");
    assert!(
        out.status.success(),
        "sandbox-api-doc-check.sh failed: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
