//! PRD-mcphost-unimplemented-method-status
//! AC1 (P0) -- Given every refusable method in `CAPABILITIES`,
//! `nonexistent/method` and `resources/subscribe`, When each is POSTed through
//! `build_router` as a 2026-07-28 request, inside a 2025-11-25 session and as
//! a 2025-11-25 request with no session, Then the status is 404, 200 and 200,
//! `resources/subscribe` on 2026-07-28 gets 404 with the labelled body, every
//! body carries `error.code` -32601 and a `data.code`, and the R9c drift test
//! passes against rmcp 3.3.0's method constants.

use crate::common;
use common::TestServer;
use mcphost::capabilities::{CAPABILITIES, CORE_METHODS, dispatches};
use rmcp::model::{
    CallToolRequestMethod, CancelTaskMethod, CompleteRequestMethod, ConstString, DiscoverRequestMethod,
    GetPromptRequestMethod, GetTaskMethod, InitializeResultMethod, ListPromptsRequestMethod,
    ListResourceTemplatesRequestMethod, ListResourcesRequestMethod, ListToolsRequestMethod, PingRequestMethod,
    ReadResourceRequestMethod, SetLevelRequestMethod, SubscribeRequestMethod, SubscriptionsListenRequestMethod,
    UnsubscribeRequestMethod, UpdateTaskMethod,
};
use serde_json::{Value, json};

const REV_NEW: &str = "2026-07-28";
const REV_OLD: &str = "2025-11-25";

/// Every method rmcp 3.3.0 dispatches as a `ClientRequest`, from its
/// `const_string!` constants (the same set `cgd_ac04` pins by variant).
const RMCP_METHODS: &[&str] = &[
    PingRequestMethod::VALUE,
    InitializeResultMethod::VALUE,
    DiscoverRequestMethod::VALUE,
    CompleteRequestMethod::VALUE,
    SetLevelRequestMethod::VALUE,
    GetPromptRequestMethod::VALUE,
    ListPromptsRequestMethod::VALUE,
    ListResourcesRequestMethod::VALUE,
    ListResourceTemplatesRequestMethod::VALUE,
    ReadResourceRequestMethod::VALUE,
    SubscriptionsListenRequestMethod::VALUE,
    SubscribeRequestMethod::VALUE,
    UnsubscribeRequestMethod::VALUE,
    CallToolRequestMethod::VALUE,
    ListToolsRequestMethod::VALUE,
    GetTaskMethod::VALUE,
    UpdateTaskMethod::VALUE,
    CancelTaskMethod::VALUE,
];

fn params_for(method: &str) -> Value {
    match method {
        "tools/call" => json!({"name": "x"}),
        "resources/read" | "resources/subscribe" | "resources/unsubscribe" => json!({"uri": "mcphost://x"}),
        "subscriptions/listen" => json!({"notifications": {"toolsListChanged": true}}),
        "prompts/get" => json!({"name": "x"}),
        "completion/complete" => {
            let body: Value = serde_json::from_str(include_str!("fixtures/clive_complete_keyless.json")).expect("fixture is JSON");
            body["params"].clone()
        }
        "logging/setLevel" => json!({"level": "info"}),
        "initialize" => json!({"protocolVersion": REV_OLD, "capabilities": {}, "clientInfo": {"name": "refst", "version": "1"}}),
        "sampling/createMessage" => json!({"messages": [], "maxTokens": 1}),
        "elicitation/create" => json!({"message": "x", "requestedSchema": {"type": "object", "properties": {}}}),
        _ => json!({}),
    }
}

/// POST one request; returns the HTTP status and the JSON-RPC reply.
async fn post(base_url: &str, method: &str, revision: &str, session: Option<&str>) -> (u16, Value) {
    let mut params = params_for(method);
    if revision >= REV_NEW {
        params["_meta"] = json!({
            "io.modelcontextprotocol/protocolVersion": revision,
            "io.modelcontextprotocol/clientCapabilities": {},
            "io.modelcontextprotocol/clientInfo": {"name": "refst", "version": "1"},
        });
    }
    let mut req = reqwest::Client::new()
        .post(format!("{base_url}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", revision)
        .header("Mcp-Method", method)
        .header("Mcp-Name", "mcphost://x")
        .body(json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string());
    if let Some(id) = session {
        req = req.header("Mcp-Session-Id", id);
    }
    let resp = req.send().await.expect("POST /mcp");
    let status = resp.status().as_u16();
    let text = resp.text().await.expect("body");
    let body = if text.trim_start().starts_with('{') {
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("reply not JSON ({e}): {text}"))
    } else {
        common::parse_sse_messages(&text).pop().unwrap_or(Value::Null)
    };
    (status, body)
}

fn refusable() -> Vec<&'static str> {
    let mut methods: Vec<&'static str> = CAPABILITIES
        .iter()
        .flat_map(|c| c.methods.iter().copied().filter(move |m| !c.implemented || c.server_to_client.contains(m)))
        .collect();
    methods.extend(["nonexistent/method", "resources/subscribe"]);
    methods.sort_unstable();
    methods.dedup();
    methods
}

#[tokio::test]
async fn refst_ac01_status_is_404_on_2026_07_28_and_200_on_2025_11_25_for_every_refusable_method() {
    let server = TestServer::start().await;
    let mut failures = Vec::new();
    for method in refusable() {
        let cells = [
            (REV_NEW, None, 404u16),
            (REV_OLD, Some("not-minted-session"), 200),
            (REV_OLD, None, 200),
        ];
        for (rev, session, want) in cells {
            let (status, reply) = post(&server.base_url, method, rev, session).await;
            let cell = format!("{method} @ {rev} session={session:?}");
            let err = &reply["error"];
            // `resources/subscribe` is dispatched on 2025-11-25: the cell proves it is NOT refused.
            if method == "resources/subscribe" && rev == REV_OLD {
                if err["code"] == -32601 {
                    failures.push(format!("{cell}: dispatched on {rev} but refused: {reply}"));
                }
            } else if status != want {
                failures.push(format!("{cell}: want HTTP {want}, got {status}: {reply}"));
            } else if err["code"] != -32601 {
                failures.push(format!("{cell}: want error.code -32601, got {reply}"));
            } else if err["data"]["code"].as_str().unwrap_or_default().is_empty() {
                failures.push(format!("{cell}: no data.code (bare body): {reply}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn refst_ac01_resources_subscribe_on_2026_07_28_is_404_with_the_labelled_body() {
    let server = TestServer::start().await;
    let (status, reply) = post(&server.base_url, "resources/subscribe", REV_NEW, None).await;
    let err = &reply["error"];
    assert_eq!(status, 404, "reply: {reply}");
    assert_eq!(err["code"], -32601, "reply: {reply}");
    assert_eq!(err["data"]["code"], "method_unknown", "reply: {reply}");
    assert_eq!(err["data"]["method"], "resources/subscribe", "reply: {reply}");
    assert!(err["data"]["help_url"].as_str().unwrap_or_default().ends_with("/help/method_unknown"), "reply: {reply}");
    // The same method on 2025-11-25 is dispatched, not refused.
    let (status, reply) = post(&server.base_url, "resources/subscribe", REV_OLD, None).await;
    assert_eq!(status, 200, "reply: {reply}");
    assert_ne!(reply["error"]["code"], -32601, "reply: {reply}");
}

/// R9c: the allowlist and rmcp's dispatch agree, per revision. Through the
/// router WITHOUT the gate, a method rmcp answers with anything but -32601 is
/// dispatched and must be in the allowlist; a method it refuses must not be.
#[tokio::test]
async fn refst_ac01_allowlist_cannot_drift_from_rmcp_dispatch() {
    let (mut state, _data_dir) = common::bare_app_state().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    state.public_url = format!("http://{addr}");
    let router = mcphost::http::build_router_without_method_gate(std::sync::Arc::new(state));
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    let base = format!("http://{addr}");
    let mut failures = Vec::new();
    for core in CORE_METHODS {
        assert!(RMCP_METHODS.contains(core), "CORE_METHODS names `{core}`, which rmcp 3.3.0 has no constant for");
    }
    for rev in [REV_OLD, REV_NEW] {
        for method in RMCP_METHODS {
            // `initialize` needs a full handshake body and `tasks/*`, `server/discover`
            // and `subscriptions/listen` carry no registry row: the allowlist side is
            // checked for them, the rmcp side by the wire only where it answers.
            let (status, reply) = post(&base, method, rev, None).await;
            let rmcp_refuses = reply["error"]["code"] == -32601;
            // A registry refusal is deliberate policy over a method rmcp may dispatch.
            if mcphost::capabilities::refusal(method, None).is_some() {
                continue;
            }
            let listed = dispatches(method, Some(rev));
            if rmcp_refuses && listed {
                failures.push(format!("{method} @ {rev}: in the allowlist but rmcp refuses it (HTTP {status}): {reply}"));
            }
            if !rmcp_refuses && !listed {
                failures.push(format!("{method} @ {rev}: rmcp dispatches it (HTTP {status}) but the allowlist omits it: {reply}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
