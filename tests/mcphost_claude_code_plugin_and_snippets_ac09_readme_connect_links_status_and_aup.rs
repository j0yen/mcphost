//! AC9 (PRD-mcphost-claude-code-plugin-and-snippets) — Given README
//! "Connect", When grepped, Then it links to `/status.html` and
//! `/aup.html`.
//!
//! mcphost-polish-p0-20260930 (audit finding 1): the README-grep checks
//! below were the ONLY gate on these two links, and they stayed green
//! while both pages 404'd live -- a string-contains check on the doc can
//! never catch the doc promising a path the server doesn't actually serve.
//! The two tests added here close that gap: they start the real
//! in-process router (`common::TestServer`, the same harness every other
//! HTTP-surface test in this suite uses) and GET the exact paths the
//! README names, so a regression that un-routes either page (or a future
//! "docs-only" edit that renames one without updating the handler) fails a
//! test instead of shipping a broken promise again.

use crate::common;
use common::TestServer;

const README: &str = include_str!("../README.md");

fn connect_section() -> &'static str {
    let start = README
        .find("## Connect")
        .unwrap_or_else(|| panic!("README.md must have a '## Connect' heading"));
    let after_heading = start + "## Connect".len();
    let end = README[after_heading..]
        .find("\n## ")
        .map(|offset| after_heading + offset)
        .unwrap_or(README.len());
    &README[start..end]
}

#[test]
fn connect_section_links_status_page() {
    assert!(
        connect_section().contains("/status.html"),
        "README '## Connect' section must link to /status.html"
    );
}

#[test]
fn connect_section_links_aup_page() {
    assert!(
        connect_section().contains("/aup.html"),
        "README '## Connect' section must link to /aup.html"
    );
}

/// Given the README's own "Connect" promise that `/status.html` works,
/// When a real request hits the in-process server at that exact path,
/// Then it answers 200 with a non-empty HTML body -- not the router's
/// default 404 the live site served when this test only checked the
/// README string.
#[tokio::test]
async fn status_html_is_served_and_renders() {
    let server = TestServer::start().await;

    let response = reqwest::get(format!("{}/status.html", server.base_url))
        .await
        .expect("GET /status.html");
    assert_eq!(response.status(), 200, "/status.html must answer 200");

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        content_type.contains("text/html"),
        "/status.html must be served as HTML, got content-type {content_type:?}"
    );

    let body = response.text().await.expect("/status.html body");
    assert!(!body.trim().is_empty(), "/status.html body must not be empty");
    assert!(
        body.to_lowercase().contains("<html"),
        "/status.html body must be real HTML, got: {body:.200}"
    );
}

/// Twin of `status_html_is_served_and_renders` for `/aup.html` -- same
/// README promise, same live-404 bug, same fix.
#[tokio::test]
async fn aup_html_is_served_and_renders() {
    let server = TestServer::start().await;

    let response = reqwest::get(format!("{}/aup.html", server.base_url))
        .await
        .expect("GET /aup.html");
    assert_eq!(response.status(), 200, "/aup.html must answer 200");

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        content_type.contains("text/html"),
        "/aup.html must be served as HTML, got content-type {content_type:?}"
    );

    let body = response.text().await.expect("/aup.html body");
    assert!(!body.trim().is_empty(), "/aup.html body must not be empty");
    assert!(
        body.to_lowercase().contains("<html"),
        "/aup.html body must be real HTML, got: {body:.200}"
    );
}
