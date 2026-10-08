//! AC2 (PRD-mcphost-llms-install-doc) — Given a running server, When
//! `GET /llms-install.md` is fetched without credentials, Then 200,
//! `Content-Type: text/markdown; charset=utf-8`, body byte-equal to the
//! generator output.

use crate::common;
use common::TestServer;
use mcphost::install_links;

#[tokio::test]
async fn llms_install_md_is_served_without_credentials_as_the_generator_output() {
    let server = TestServer::start().await;
    let response = reqwest::Client::new()
        .get(format!("{}/llms-install.md", server.base_url))
        .send()
        .await
        .expect("GET /llms-install.md");

    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers().get("content-type").and_then(|v| v.to_str().ok()),
        Some("text/markdown; charset=utf-8")
    );
    let cache = response.headers().get("cache-control").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
    assert!(cache.contains("max-age=600"), "ten-minute cache, got {cache:?}");

    let body = response.bytes().await.expect("body");
    let expected = install_links::render_install_doc(&server.state.public_url);
    assert_eq!(body.as_ref(), expected.as_bytes(), "served bytes must equal the generator output");
}
