//! PRD-mcphost-first-hour-support-surface
//! AC7 (P1) — Given `host.whoami`, When answered, Then
//! `links.support|plans|status|help` are present and each resolves on the
//! in-process server.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn whoami_links_are_present_and_all_resolve() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Links Reader").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let result = client
        .tools_call("host.whoami", json!({}))
        .await
        .expect("host.whoami should succeed for a signed-up tenant");
    let structured = extract_structured(&result);

    let links = structured["links"].as_object().expect("links object");
    for key in ["support", "plans", "status", "help"] {
        assert!(
            links.contains_key(key),
            "host.whoami links must include {key}: {links:?}"
        );
    }

    let http = reqwest::Client::new();
    for (name, url) in [
        ("support", links["support"].as_str().unwrap()),
        ("plans", links["plans"].as_str().unwrap()),
        ("status", links["status"].as_str().unwrap()),
        ("help", links["help"].as_str().unwrap()),
    ] {
        assert!(url.starts_with(&server.base_url), "{name} link must resolve on this server: {url}");
        let resp = http.get(url).send().await.unwrap_or_else(|e| panic!("GET {name} link {url}: {e}"));
        assert_eq!(resp.status(), reqwest::StatusCode::OK, "{name} link {url} must resolve 200");
        let body = resp.text().await.unwrap_or_default();
        assert!(!body.is_empty(), "{name} link {url} must have a non-empty body");
    }
}
