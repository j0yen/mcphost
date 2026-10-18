//! PRD-mcphost-capabilities-generated-from-dispatch
//! AC1 (P0) -- Given the in-process server with the gate absent, When
//! `roots/list` with `params: {}` and no `Mcp-Method` header is POSTed keyless
//! to `/mcp`, Then the reply is `-32601` with no `data` member (the premise);
//! Given the gate installed, Then the same request answers `-32601` with
//! `data.code == "capability_unsupported"`, `data.capability == "roots"` and
//! `data.help_url` ending `/help/capability_unsupported`.

use crate::common;
use common::TestServer;
use serde_json::{Value, json};

async fn post_roots_list(base_url: &str) -> Value {
    reqwest::Client::new()
        .post(format!("{base_url}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(json!({"jsonrpc": "2.0", "id": 1, "method": "roots/list", "params": {}}).to_string())
        .send()
        .await
        .expect("POST /mcp")
        .json()
        .await
        .expect("JSON reply")
}

#[tokio::test]
async fn cgd_ac01_premise_gate_absent_answers_bare_minus_32601() {
    let (mut state, _data_dir) = common::bare_app_state().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    state.public_url = format!("http://{addr}");
    let router = mcphost::http::build_router_without_method_gate(std::sync::Arc::new(state));
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    let reply = post_roots_list(&format!("http://{addr}")).await;
    assert_eq!(reply["error"]["code"], -32601, "reply: {reply}");
    assert!(reply["error"].get("data").is_none(), "premise: no data member, reply: {reply}");
}

#[tokio::test]
async fn cgd_ac01_gate_installed_labels_roots_list() {
    let server = TestServer::start().await;
    let reply = post_roots_list(&server.base_url).await;
    let err = &reply["error"];
    assert_eq!(err["code"], -32601, "reply: {reply}");
    assert_eq!(err["data"]["code"], "capability_unsupported", "reply: {reply}");
    assert_eq!(err["data"]["capability"], "roots", "reply: {reply}");
    assert!(
        err["data"]["help_url"].as_str().unwrap_or_default().ends_with("/help/capability_unsupported"),
        "reply: {reply}"
    );
}
