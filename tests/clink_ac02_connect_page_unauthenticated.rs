//! AC2 (PRD-mcphost-client-install-links) — Given an unauthenticated GET
//! `/connect`, When rendered, Then the page contains all four forms
//! pointing at `{public_url}/mcp`, without JavaScript, with HTTP 200.

use crate::common;
use common::TestServer;
use mcphost::install_links;

#[tokio::test]
async fn connect_renders_all_four_forms_with_no_auth_and_no_js() {
    let server = TestServer::start().await;
    let links = install_links::for_url(&server.state.public_url);

    let response = reqwest::Client::new()
        .get(format!("{}/connect", server.base_url))
        .send()
        .await
        .expect("GET /connect");

    assert_eq!(response.status(), 200, "/connect must answer 200 with no Authorization header");

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(content_type.contains("text/html"), "got content-type {content_type:?}");

    let body = response.text().await.expect("/connect body");

    // Cursor/VS Code link through `/connect/go/<client>` (AC5's own
    // redirect-and-record wrapper), not the deep link directly -- AC5's
    // own test proves that route actually 302s on to `links.cursor`.
    assert!(body.contains(&links.mcp_url), "body must show the plain endpoint: {body}");
    assert!(body.contains("/connect/go/cursor"), "body must link Cursor through /connect/go/cursor: {body}");
    assert!(body.contains("/connect/go/vscode"), "body must link VS Code through /connect/go/vscode: {body}");
    assert!(
        body.contains(&links.claude_code_command),
        "body must carry the Claude Code command: {body}"
    );
    for step in &links.claude_ai_steps {
        assert!(body.contains(step), "body must carry Claude.ai step {step:?}: {body}");
    }

    assert!(
        !body.to_lowercase().contains("<script"),
        "/connect must render without JavaScript: {body}"
    );
}
