//! PRD-mcphost-emitted-doc-pointers-resolve
//! AC3 (P0) — Given `host.help.get {anchor: "README.md#call-limits"}`
//! anonymously, When called, Then `text` equals that section's body byte for
//! byte and `example` is its first fenced block; Given `anchor:
//! "README.md#call-limit"`, Then `anchor_not_found` with `did_you_mean`
//! containing the correct anchor.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;
use std::path::Path;

/// `(body, first fenced block)` of the `## <title>` section of README.md,
/// read straight off disk with its own tiny parser (not the build script's):
/// the body runs from the line after the heading to the next heading of the
/// same or higher level, trimmed.
fn readme_section(title: &str) -> (String, Option<(String, String)>) {
    let text = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md")).expect("README.md");
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.iter().position(|l| *l == format!("## {title}")).expect("heading");
    let mut end = lines.len();
    let mut in_fence = false;
    for (i, l) in lines.iter().enumerate().skip(start + 1) {
        if l.trim_start().starts_with("```") {
            in_fence = !in_fence;
        } else if !in_fence && (l.starts_with("# ") || l.starts_with("## ")) {
            end = i;
            break;
        }
    }
    let body = lines[start + 1..end].join("\n").trim().to_string();
    let mut example = None;
    let mut open: Option<(String, Vec<&str>)> = None;
    for l in &lines[start + 1..end] {
        match (&mut open, l.trim_start().strip_prefix("```")) {
            (None, Some(info)) => open = Some((info.split_whitespace().next().unwrap_or("").to_string(), vec![])),
            (Some((lang, code)), Some(_)) => {
                example = Some((lang.clone(), code.join("\n")));
                break;
            }
            (Some((_, code)), None) => code.push(l),
            (None, None) => {}
        }
    }
    (body, example)
}

#[tokio::test]
async fn edp_ac03_call_limits_text_is_the_section_body_byte_for_byte() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);
    let result = client.tools_call("host.help.get", json!({"anchor": "README.md#call-limits"})).await.expect("host.help.get anonymously");
    let body = extract_structured(&result);
    let (want, example) = readme_section("Call limits");
    assert!(want.len() <= 4096, "the section fits the 4 KB cap, else the cut is what is compared");
    assert_eq!(body["text"].as_str(), Some(want.as_str()));
    assert_eq!(body["anchor"], "README.md#call-limits");
    assert_eq!(body["path"], "README.md");
    assert!(body["line"].as_u64().unwrap_or(0) > 0, "{body}");
    match example {
        Some((lang, code)) => assert_eq!(body["example"], json!({"lang": lang, "code": code})),
        None => assert!(body.get("example").is_none(), "no fenced block, no example: {body}"),
    }
}

#[tokio::test]
async fn edp_ac03_example_is_the_first_fenced_block() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);
    let result = client.tools_call("host.help.get", json!({"anchor": "README.md#quickstart-for-agents"})).await.expect("host.help.get");
    let body = extract_structured(&result);
    let (_, example) = readme_section("Quickstart for agents");
    let (lang, code) = example.expect("the quickstart section has a fenced block");
    assert_eq!(body["example"], json!({"lang": lang, "code": code}), "{body}");
}

#[tokio::test]
async fn edp_ac03_misspelled_anchor_is_anchor_not_found_with_did_you_mean() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);
    let err = client
        .tools_call("host.help.get", json!({"anchor": "README.md#call-limit"}))
        .await
        .expect_err("an unknown anchor is refused");
    assert_eq!(err.error_code.as_deref(), Some("anchor_not_found"), "{err:?}");
    let near = err.data["did_you_mean"].as_array().expect("did_you_mean");
    assert!(near.len() <= 3, "{near:?}");
    assert!(near.iter().any(|a| a == "README.md#call-limits"), "{near:?}");
    assert_eq!(err.data["hint"], "host.help.search");
}
