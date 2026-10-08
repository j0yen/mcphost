//! AC5 (PRD-mcphost-install-links-more-clients) — Given an authenticated
//! tenant, When it calls `host.quickstart`, Then `install_links` has 12
//! entries whose artefacts contain that tenant's `/u/<secret>/mcp` URL and
//! none contain `/mcp` bare.
//!
//! `install_links.surfaces` is the 12-entry table; the original keys
//! (`mcp_url`, `cursor`, ...) stay alongside it for compatibility.

use crate::common;
use base64::Engine as _;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(byte) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or(""), 16)
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The artefact with deep-link encoding undone, so the URL it carries can
/// be read back the way the client's own handler would.
fn decoded(id: &str, artefact: &str) -> String {
    match id {
        "cursor" => {
            let b64 = percent_decode(artefact.split("config=").nth(1).expect("config param"));
            let raw = base64::engine::general_purpose::STANDARD.decode(b64).expect("base64");
            String::from_utf8(raw).expect("utf8")
        }
        "vscode" => percent_decode(artefact.split_once('?').expect("query").1),
        _ => artefact.to_string(),
    }
}

#[tokio::test]
async fn quickstart_install_links_carry_twelve_personal_artefacts() {
    let server = TestServer::start().await;
    let session = McpClient::new(&server.base_url).with_session_continuity();
    session.tools_call("signup", json!({"name": "AC5 Tenant"})).await.expect("signup");
    let rotated = extract_structured(&session.tools_call("host.key_rotate", json!({})).await.expect("rotate"));
    let url = rotated["url"].as_str().expect("url").to_string();
    let path = url.trim_start_matches(&server.base_url).to_string();

    let quickstart = extract_structured(
        &McpClient::new(&server.base_url)
            .with_path(&path)
            .tools_call("host.quickstart", json!({"kind": "echo"}))
            .await
            .expect("host.quickstart"),
    );
    let surfaces = quickstart["install_links"]["surfaces"].as_array().expect("surfaces array");
    assert_eq!(surfaces.len(), 12, "{quickstart}");

    let personal_prefix = url.trim_end_matches("/mcp").to_string();
    for s in surfaces {
        let id = s["id"].as_str().expect("id");
        let text = decoded(id, s["artefact"].as_str().expect("artefact"));
        assert!(text.contains(&url), "{id} must carry the personal URL {url}: {text}");
        // Every `/mcp` occurrence must be the end of the personal URL.
        assert_eq!(
            text.matches("/mcp").count(),
            text.matches(&format!("{personal_prefix}/mcp")).count(),
            "{id} carries a bare /mcp: {text}"
        );
    }
}
