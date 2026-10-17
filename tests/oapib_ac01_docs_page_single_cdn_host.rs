//! PRD-mcphost-openapi-for-rest-facade-b
//! AC1 (P0) -- Given `GET /docs`, When fetched, Then it is 200 HTML whose only
//! external `<script src>` host is `cdnjs.cloudflare.com` and which references
//! `/openapi.json`; `scripts/www-check.sh` exits 0.

use crate::common::TestServer;
use std::process::Command;

/// Hosts of every `<script ... src="...">` URL in `html` that is not same-origin.
fn external_script_hosts(html: &str) -> Vec<String> {
    let mut hosts = Vec::new();
    let mut rest = html;
    while let Some(i) = rest.find("<script") {
        let tail = &rest[i..];
        let tag_end = tail.find('>').unwrap_or(tail.len());
        let tag = &tail[..tag_end];
        if let Some(s) = tag.find("src=") {
            let val = tag[s + 4..].trim_start_matches(['"', '\'']);
            let url = val.split(['"', '\'', ' ']).next().unwrap_or("");
            if let Some(after) = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")).or_else(|| url.strip_prefix("//")) {
                hosts.push(after.split('/').next().unwrap_or("").to_string());
            }
        }
        rest = &tail[tag_end..];
    }
    hosts
}

#[tokio::test]
async fn oapib_ac01() {
    let server = TestServer::start().await;
    let resp = reqwest::get(format!("{}/docs", server.base_url)).await.expect("GET /docs");
    assert_eq!(resp.status(), 200);
    let ctype = resp.headers()["content-type"].to_str().expect("content-type ascii").to_string();
    assert!(ctype.starts_with("text/html"), "content-type was {ctype}");
    let html = resp.text().await.expect("body");

    let hosts = external_script_hosts(&html);
    assert!(!hosts.is_empty(), "the Redoc bundle must be loaded");
    assert!(hosts.iter().all(|h| h == "cdnjs.cloudflare.com"), "external script hosts: {hosts:?}");
    assert!(html.contains("/openapi.json"), "/docs must reference /openapi.json");

    // The scanner is not vacuous: a second host is caught.
    let bad = format!("{html}<script src=\"https://evil.example/x.js\"></script>");
    assert!(external_script_hosts(&bad).iter().any(|h| h == "evil.example"));

    let status = Command::new("bash")
        .arg("scripts/www-check.sh")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .status()
        .expect("run www-check.sh");
    assert!(status.success(), "scripts/www-check.sh exited {status}");
}
