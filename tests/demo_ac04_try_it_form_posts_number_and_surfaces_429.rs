//! PRD-mcphost-demo-url-per-tool
//! AC4 (P0) — Given the try-it form on a tool with an integer argument,
//! When "7" is submitted, Then the call reaches `/x/{token}/{tool}` with `7`
//! as a number and the result renders inline; Given the share caller limit
//! is exhausted, Then the form shows the same 429 body `/x/` returns.
//!
//! The form submits client-side (fetch); this test reads the form the page
//! serves, checks the script's coercion + inline rendering are present, then
//! issues the exact request the script builds.

use crate::common;
use common::{TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn try_it_form_posts_integer_as_number_and_shows_the_429_body() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Demo AC4").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "echo",
                "kind": "echo",
                "spec": {"schema": {"type": "object", "properties": {"n": {"type": "integer"}}, "required": ["n"]}},
            }),
        )
        .await
        .expect("publish");
    let shared = extract_structured(
        &client
            .tools_call("host.tool_share", json!({"name": "echo", "visibility": "url"}))
            .await
            .expect("share"),
    );
    let url = shared["url"].as_str().expect("url").to_string();
    let token_path = url.strip_prefix(&server.base_url).expect("same origin").to_string();

    let http = reqwest::Client::new();
    let html = http
        .get(format!("{}/d/{}/echo", server.base_url, ns))
        .send()
        .await
        .expect("GET /d")
        .text()
        .await
        .unwrap();
    assert!(
        html.contains(&format!("<form id=\"try\" action=\"{token_path}\" method=\"post\">")),
        "form posts to /x/{{token}}/{{tool}}: {html}"
    );
    assert!(html.contains("name=\"n\" data-type=\"integer\""), "integer field: {html}");
    assert!(html.contains("parseInt(v,10)"), "script coerces integers to numbers");
    assert!(html.contains("out.textContent=r.status+' '+await r.text()"), "result/body rendered inline");

    // What the script sends for "7": a JSON number.
    let resp = http.post(&url).header("content-type", "application/json").body(json!({"n": 7}).to_string()).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["result"]["n"].is_i64(), "7 reached the tool as a number: {body}");
    assert_eq!(body["result"]["n"], json!(7));

    // Exhaust the per-address limit: the 429 body is exactly what /x/ returns.
    let mut last = None;
    for _ in 0..80 {
        let r = http.post(&url).header("content-type", "application/json").body(r#"{"n":7}"#).send().await.unwrap();
        if r.status() == 429 {
            last = Some(r.text().await.unwrap());
            break;
        }
    }
    let body = last.expect("limit exhausted -> 429");
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["error"]["code"], json!("rate_limited"));
}
