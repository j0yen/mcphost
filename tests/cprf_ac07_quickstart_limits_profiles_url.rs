//! PRD-mcphost-client-profile-registry AC7 — Given an anonymous
//! `host.quickstart`, When it answers, Then `limits.client_profiles_url` is
//! the absolute `/client-profiles.json` URL and `limits.name_pattern` is
//! unchanged.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn anonymous_quickstart_limits_carry_the_profiles_url() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);
    let result = client.tools_call("host.quickstart", json!({})).await.expect("quickstart");
    let limits = &extract_structured(&result)["limits"];
    let url = limits["client_profiles_url"].as_str().expect("client_profiles_url");
    assert_eq!(url, format!("{}/client-profiles.json", server.base_url.trim_end_matches('/')));
    assert!(url.starts_with("http"), "absolute: {url}");
    assert_eq!(limits["name_pattern"], json!("^[a-z][a-z0-9_]{1,40}$"));
    // The URL it names answers.
    let served = reqwest::get(url).await.expect("GET").status();
    assert_eq!(served, 200);
}
