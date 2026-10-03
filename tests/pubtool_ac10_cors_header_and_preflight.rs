//! PRD-mcphost-public-tool-url
//! AC10 (P1) — Given a browser page on another origin, When it `fetch`es
//! the URL with `GET`, Then the response carries
//! `Access-Control-Allow-Origin: *` and the preflight `OPTIONS` returns
//! 204.

use crate::common;
use common::{TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn get_response_carries_acao_star_and_options_preflight_is_204() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC10 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "echo", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish ok");
    let shared = extract_structured(
        &client
            .tools_call("host.tool_share", json!({"name": "echo", "visibility": "url"}))
            .await
            .expect("tool_share ok"),
    );
    let url = shared["url"].as_str().expect("url field").to_string();

    let http = reqwest::Client::new();

    let get_resp = http.get(&url).send().await.expect("GET");
    assert_eq!(get_resp.status(), 200);
    assert_eq!(
        get_resp.headers().get("access-control-allow-origin").and_then(|v| v.to_str().ok()),
        Some("*"),
        "GET response must carry Access-Control-Allow-Origin: *"
    );

    let preflight_resp = http
        .request(reqwest::Method::OPTIONS, &url)
        .header("Origin", "https://example.com")
        .header("Access-Control-Request-Method", "GET")
        .send()
        .await
        .expect("OPTIONS");
    assert_eq!(preflight_resp.status(), 204, "preflight OPTIONS must answer 204");
    assert_eq!(
        preflight_resp
            .headers()
            .get("access-control-allow-origin")
            .and_then(|v| v.to_str().ok()),
        Some("*"),
        "preflight response must also carry Access-Control-Allow-Origin: *"
    );
}
