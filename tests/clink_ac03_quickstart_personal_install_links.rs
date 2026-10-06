//! AC3 (PRD-mcphost-client-install-links) — Given an authenticated tenant,
//! When it calls `host_quickstart`, Then the response has `install_links`
//! whose URLs contain the caller's personal `/u/<secret>/mcp` path.
//!
//! The one connection this host can show a REAL personal URL over without
//! a DB write is a reconnect through `/u/{secret}/mcp` itself (see
//! `control::quickstart`'s own doc comment on `path_url_secret`) -- the
//! exact scenario the PRD's own "Tenant (any client)" user story
//! describes ("a second machine connects pre-linked"), and the same
//! `host.key_rotate` -> `/u/{secret}/mcp` reconnect shape
//! `urltenant_ac06_quickstart_url_bound_no_signup_step.rs` already uses.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn quickstart_over_a_url_bound_session_carries_personal_install_links() {
    let server = TestServer::start().await;
    let session = McpClient::new(&server.base_url).with_session_continuity();
    session
        .tools_call("signup", json!({"name": "AC3 Tenant"}))
        .await
        .expect("signup");
    let rotated = extract_structured(
        &session
            .tools_call("host.key_rotate", json!({}))
            .await
            .expect("host.key_rotate mints a URL"),
    );
    let url = rotated["url"].as_str().expect("url").to_string();
    assert!(url.contains("/u/") && url.ends_with("/mcp"), "{url}");
    let path = url.trim_start_matches(&server.base_url).to_string();

    let url_client = McpClient::new(&server.base_url).with_path(&path);
    let quickstart = extract_structured(
        &url_client
            .tools_call("host.quickstart", json!({"kind": "echo"}))
            .await
            .expect("host.quickstart over a URL-bound session"),
    );

    let install_links = &quickstart["install_links"];
    assert!(!install_links.is_null(), "{quickstart}");

    let mcp_url = install_links["mcp_url"].as_str().expect("install_links.mcp_url");
    assert_eq!(mcp_url, url, "install_links.mcp_url must be this tenant's own personal URL: {quickstart}");

    let claude_code_command = install_links["claude_code_command"].as_str().expect("claude_code_command");
    assert!(
        claude_code_command.contains(&url),
        "Claude Code command must carry the personal URL: {claude_code_command}"
    );

    let claude_ai_steps = install_links["claude_ai_steps"].as_array().expect("claude_ai_steps");
    assert!(
        claude_ai_steps.iter().any(|s| s.as_str().is_some_and(|s| s.contains(&url))),
        "a Claude.ai step must name the personal URL: {quickstart}"
    );

    // Cursor/VS Code encode the URL inside a JSON config -- decode back out
    // rather than substring-matching an encoded blob.
    let cursor = install_links["cursor"].as_str().expect("cursor");
    let config_b64 = cursor.split("config=").nth(1).expect("cursor link has a config param");
    let config_b64 = urlpercent_decode(config_b64);
    use base64::Engine as _;
    let config_json = base64::engine::general_purpose::STANDARD
        .decode(config_b64)
        .expect("cursor config is valid base64");
    let config_json: serde_json::Value =
        serde_json::from_slice(&config_json).expect("cursor config is valid json");
    assert_eq!(config_json["url"], json!(url), "{quickstart}");
}

/// Minimal percent-decoder for the one query value this test needs to
/// round-trip -- `install_links::for_url`'s own encoder is the thing under
/// test, so this deliberately doesn't call back into it.
fn urlpercent_decode(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}
