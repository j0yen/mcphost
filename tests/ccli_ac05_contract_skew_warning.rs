//! AC5 (PRD-mcphost-call-cli-from-contract) -- Given a test host whose
//! `/contract.json` ETag differs from the embedded sha, When any `call` runs,
//! Then stderr's first line is `contract skew: binary 1.<12> host 1.<12>` and
//! the call still proceeds; with `--no-skew-check` no HEAD is sent.

use mcphost::call_cli::{EMBEDDED_CONTRACT, contract};
use mcphost::api_contract::sha_hex;
use serde_json::json;
use tokio::process::Command;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const OTHER_SHA: &str = "b71e0c4d9a2f5e83171c6d0a4b9f2e75c3d8a1960e47fb25d3c8a9e01f64b7d2";

async fn host_with_etag(etag: &str) -> MockServer {
    let host = MockServer::start().await;
    Mock::given(method("HEAD"))
        .and(path("/contract.json"))
        .respond_with(ResponseTemplate::new(200).insert_header("ETag", format!("\"{etag}\"").as_str()))
        .mount(&host)
        .await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0", "id": 1,
            "result": {"structuredContent": {"tenant": "t_x", "ok": true}}
        })))
        .mount(&host)
        .await;
    host
}

async fn call(host: &MockServer, extra: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["call", "host.whoami", "--json", "--url", &host.uri()])
        .args(extra)
        .env_remove("MCPHOST_KEY")
        .output()
        .await
        .expect("run mcphost call")
}

fn heads(requests: &[wiremock::Request]) -> usize {
    requests.iter().filter(|r| r.method.as_str() == "HEAD").count()
}

#[tokio::test]
async fn a_differing_etag_prints_the_skew_line_first_and_the_call_proceeds() {
    let host = host_with_etag(OTHER_SHA).await;
    let out = call(&host, &[]).await;

    let stderr = String::from_utf8_lossy(&out.stderr);
    let binary = format!("1.{}", &sha_hex(EMBEDDED_CONTRACT)[..12]);
    assert_eq!(binary, contract().version, "the binary's contract version string");
    assert_eq!(
        stderr.lines().next(),
        Some(format!("contract skew: binary {binary} host 1.{}", &OTHER_SHA[..12]).as_str()),
        "stderr: {stderr}"
    );
    assert_eq!(out.status.code(), Some(0), "the call still proceeds: {stderr}");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap(),
        json!({"tenant": "t_x", "ok": true})
    );
    let requests = host.received_requests().await.unwrap();
    assert_eq!(heads(&requests), 1, "exactly one HEAD");
    assert!(requests.iter().any(|r| r.method.as_str() == "POST"), "and the tools/call");
}

#[tokio::test]
async fn no_skew_check_sends_no_head() {
    let host = host_with_etag(OTHER_SHA).await;
    let out = call(&host, &["--no-skew-check"]).await;
    assert_eq!(out.status.code(), Some(0));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("contract skew"));
    assert_eq!(heads(&host.received_requests().await.unwrap()), 0, "no HEAD with --no-skew-check");
}

#[tokio::test]
async fn a_matching_etag_is_silent() {
    let host = host_with_etag(&sha_hex(EMBEDDED_CONTRACT)).await;
    let out = call(&host, &[]).await;
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stderr.is_empty(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(heads(&host.received_requests().await.unwrap()), 1);
}
