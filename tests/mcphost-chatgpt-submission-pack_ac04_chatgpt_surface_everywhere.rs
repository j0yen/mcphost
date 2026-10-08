//! AC4 (PRD-mcphost-chatgpt-submission-pack) — Given `install_links::for_url`,
//! When inspected, Then a `chatgpt` surface of kind `steps` exists with a
//! `doc_url` and `checked` date, and `/connect`, `host.quickstart.install_links`,
//! and `/llms-install.md` all render it.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use mcphost::install_links::{self, Kind};
use serde_json::json;

#[test]
fn table_has_a_chatgpt_steps_row_with_doc_url_and_checked_date() {
    let table = install_links::for_url("https://mcphost.dev");
    let row = table.iter().find(|s| s.id == "chatgpt").expect("chatgpt surface");
    assert_eq!(row.kind, Kind::Steps);
    assert!(row.doc_url.starts_with("https://"), "{row:?}");
    assert_eq!(row.checked.len(), 10, "YYYY-MM-DD: {row:?}");
    assert!(row.artefact.contains("https://mcphost.dev/mcp"), "{row:?}");
    for step in ["Plugins", "Add", "Create custom MCP server"] {
        assert!(row.artefact.contains(step), "step names {step:?}: {row:?}");
    }
    assert!(install_links::SURFACE_IDS.contains(&"chatgpt"));
}

#[tokio::test]
async fn connect_quickstart_and_llms_install_render_the_chatgpt_row() {
    let server = TestServer::start().await;
    let row = install_links::surface_for(&server.state.public_url, "chatgpt").expect("chatgpt surface");
    let http = reqwest::Client::new();

    let connect = http.get(format!("{}/connect", server.base_url)).send().await.expect("GET /connect");
    assert_eq!(connect.status(), 200);
    let body = connect.text().await.expect("body");
    assert_eq!(body.matches("id=\"chatgpt\"").count(), 1, "{body}");
    assert!(body.contains("Create custom MCP server"), "{body}");

    let session = McpClient::new(&server.base_url).with_session_continuity();
    session.tools_call("signup", json!({"name": "ChatGPT Pack"})).await.expect("signup");
    let rotated = extract_structured(&session.tools_call("host.key_rotate", json!({})).await.expect("rotate"));
    let path = rotated["url"].as_str().expect("url").trim_start_matches(&server.base_url).to_string();
    let quickstart = extract_structured(
        &McpClient::new(&server.base_url)
            .with_path(&path)
            .tools_call("host.quickstart", json!({"kind": "echo"}))
            .await
            .expect("host.quickstart"),
    );
    let text = quickstart.to_string();
    let rows = quickstart["install_links"]["surfaces"].as_array().expect("install_links.surfaces");
    let chatgpt = rows.iter().find(|s| s["id"] == "chatgpt").unwrap_or_else(|| panic!("chatgpt row in {text}"));
    assert_eq!(chatgpt["kind"], "steps");
    assert_eq!(chatgpt["doc_url"], row.doc_url);
    assert_eq!(chatgpt["checked"], row.checked);

    // `/llms-install.md` is rendered from the same table; it must exist
    // (200, no 404 escape hatch) and carry the chatgpt row's steps and docs.
    let doc = http.get(format!("{}/llms-install.md", server.base_url)).send().await.expect("GET /llms-install.md");
    assert_eq!(doc.status(), 200, "/llms-install.md must be served");
    let md = doc.text().await.expect("llms-install.md body");
    assert!(md.contains("**ChatGPT") || md.contains(row.label), "{md}");
    assert!(md.contains("Create custom MCP server"), "{md}");
    assert!(md.contains(row.doc_url), "{md}");
}
