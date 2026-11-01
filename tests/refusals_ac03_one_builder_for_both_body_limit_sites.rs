//! PRD-mcphost-refusals-name-their-cause AC3 -- Given both body-limit sites
//! (`refuse_unimplemented_methods`, `deliver_client_response` in
//! `src/http.rs`), When either refuses, Then one shared builder writes the
//! § 413 body: a unit test covers `Some(content_length)` and `None`, HTTP
//! tests at `refuse_unimplemented_methods` cover 1,048,577 bytes with
//! `Content-Length` and a chunked body (`content_length` null),
//! `deliver_client_response` calls that builder, and `drain_gate` keeps its
//! 503 JSON.

use crate::common;

use common::TestServer;
use mcphost::http::{request_body_too_large, request_body_too_large_body};
use mcphost::state::MAX_REQUEST_BODY_BYTES;
use serde_json::{Value, json};

const HTTP_RS: &str = include_str!("../src/http.rs");

/// The text of `async fn <name>` up to the next top-level closing brace.
fn fn_body(name: &str) -> &'static str {
    let start = HTTP_RS.find(&format!("async fn {name}")).unwrap_or_else(|| panic!("no async fn {name}"));
    let rest = &HTTP_RS[start..];
    &rest[..rest.find("\n}\n").expect("fn end")]
}

#[test]
fn builder_covers_a_declared_length_and_a_chunked_request() {
    let some = request_body_too_large_body("https://host.test", Some(1_048_577));
    assert_eq!(some["jsonrpc"], "2.0");
    assert_eq!(some["id"], Value::Null);
    assert_eq!(some["error"]["code"], -32600);
    assert_eq!(some["error"]["message"], "request body too large: 1048577 bytes, limit 1048576");
    assert_eq!(some["error"]["data"]["content_length"], 1_048_577);
    assert_eq!(some["error"]["data"]["limit_bytes"], MAX_REQUEST_BODY_BYTES);
    assert_eq!(some["error"]["data"]["error_code"], "request_body_too_large");
    assert_eq!(
        some["error"]["data"]["hint"],
        "send less than 1 MiB per call; store a large payload with host.blob_put and pass its blob id"
    );

    let none = request_body_too_large_body("https://host.test", None);
    assert_eq!(none["error"]["message"], "request body too large: over 1048576 bytes");
    assert!(none["error"]["data"]["content_length"].is_null(), "{none}");
    assert!(none["error"]["data"].as_object().unwrap().contains_key("content_length"), "null, not absent");
    assert_eq!(none["error"]["data"]["help_url"], "https://host.test/help/request_body_too_large");
}

#[tokio::test]
async fn builder_response_is_413_application_json() {
    let resp = request_body_too_large("https://host.test", Some(2_000_000));
    assert_eq!(resp.status().as_u16(), 413);
    let content_type = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("");
    assert!(content_type.starts_with("application/json"), "{content_type}");
}

async fn assert_refused(resp: reqwest::Response, content_length: Value) {
    assert_eq!(resp.status().as_u16(), 413);
    let content_type = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    assert!(content_type.starts_with("application/json"), "{content_type}");
    let body: Value = resp.json().await.expect("JSON 413 body");
    assert_eq!(body["error"]["data"]["error_code"], "request_body_too_large", "{body}");
    assert_eq!(body["error"]["data"]["content_length"], content_length, "{body}");
    assert_eq!(body["error"]["data"]["limit_bytes"], MAX_REQUEST_BODY_BYTES);
}

fn request(base: &str) -> reqwest::RequestBuilder {
    reqwest::Client::new()
        .post(format!("{base}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
}

fn oversize_json() -> String {
    let pre = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{"p":""#;
    let suf = r#""}}"#;
    format!("{pre}{}{suf}", "x".repeat(MAX_REQUEST_BODY_BYTES + 1 - pre.len() - suf.len()))
}

fn chunked_oversize() -> reqwest::Body {
    let chunks: Vec<Result<Vec<u8>, std::io::Error>> = (0..(MAX_REQUEST_BODY_BYTES / 65_536))
        .map(|_| Ok(vec![b'x'; 65_536]))
        .chain(std::iter::once(Ok(vec![b'x'; 1])))
        .collect();
    reqwest::Body::wrap_stream(futures_util::stream::iter(chunks))
}

#[tokio::test]
async fn refuse_unimplemented_methods_answers_the_shared_body() {
    let server = TestServer::start().await;
    let body = oversize_json();
    assert_eq!(body.len(), MAX_REQUEST_BODY_BYTES + 1);

    let declared = request(&server.base_url).body(body).send().await.expect("send");
    assert_refused(declared, json!(MAX_REQUEST_BODY_BYTES + 1)).await;

    let chunked = request(&server.base_url).body(chunked_oversize()).send().await.expect("send chunked");
    assert_refused(chunked, Value::Null).await;
}

#[tokio::test]
async fn the_limit_refusal_does_not_depend_on_the_method_gate() {
    let (mut state, _data_dir) = common::bare_app_state().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    state.public_url = format!("http://{addr}");
    let router = mcphost::http::build_router_without_method_gate(std::sync::Arc::new(state));
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    let resp = request(&format!("http://{addr}")).body(oversize_json()).send().await.expect("send");
    assert_refused(resp, json!(MAX_REQUEST_BODY_BYTES + 1)).await;
}

#[test]
fn both_sites_call_the_one_builder_and_drain_gate_keeps_its_503() {
    for site in ["refuse_unimplemented_methods", "deliver_client_response"] {
        let body = fn_body(site);
        assert!(body.contains("request_body_too_large("), "{site} must call the shared builder");
        assert!(!body.contains("PAYLOAD_TOO_LARGE"), "{site} must not write its own 413");
    }
    assert_eq!(
        HTTP_RS.matches("StatusCode::PAYLOAD_TOO_LARGE").count(),
        1,
        "exactly one place writes the 413 status: the shared builder"
    );
    let drain = fn_body("drain_gate");
    assert!(drain.contains("StatusCode::SERVICE_UNAVAILABLE"), "{drain}");
    assert!(drain.contains(r#""state": "draining""#), "{drain}");
    assert!(!drain.contains("request_body_too_large"), "the drain 503 is not the body-limit refusal");
}
