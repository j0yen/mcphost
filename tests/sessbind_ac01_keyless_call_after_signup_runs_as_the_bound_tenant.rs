//! PRD-mcphost-session-bound-tenant-after-signup
//! AC1 (P0) — Given a streamable-HTTP session that ran `signup`
//! successfully, When it calls `host.catalog.search` with no `tenant_key`
//! and no `Authorization` header, Then the call succeeds as the new tenant
//! and no response on that session carries `error_code`
//! `tenant_key_missing`.
//!
//! The session identity this AC turns on is minted by mcphost itself
//! (`http::issue_session_id` + `session_bind::SessionBindings`), not by
//! `rmcp`'s own session mode. The second test here is why: flipping
//! `legacy_session_mode(true)` -- `rmcp` 3.3.0's only built-in
//! `Mcp-Session-Id` mechanism -- makes the transport *require* the header
//! back on every follow-up request, and the Claude Agent SDK's own recorded
//! wire sequence (`tests/compat_ac11_ac12_claude_sdk_replay.rs`) never sends
//! one, so that shipped compat guarantee would break. Minting the id a layer
//! above the transport keeps both: a client that echoes the header gets a
//! bound session, a client that ignores it sees v0.60.35 behaviour exactly.

use crate::common;
use common::{McpClient, TestServer, extract_structured, parse_response_body};
use serde_json::{Value, json};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

/// AC1's own Given/When/Then, end to end over the wire.
#[tokio::test]
async fn signed_up_session_runs_a_keyless_catalog_search_as_the_new_tenant() {
    let server = TestServer::start().await;

    // One client, one session -- it echoes the `Mcp-Session-Id` the server
    // issued, which is what a spec-compliant streamable-HTTP client does.
    let session = McpClient::new(&server.base_url).with_session_continuity();

    let signed_up = session
        .tools_call("signup", json!({"name": "AC1 Tenant"}))
        .await
        .expect("signup");
    let signed_up = extract_structured(&signed_up);
    let namespace = signed_up["tenant"].as_str().expect("tenant namespace").to_string();

    // When: no `tenant_key` argument, no `Authorization` header.
    let searched = session
        .tools_call("host.catalog.search", json!({}))
        .await
        .expect("a key-less catalog.search on the session that just signed up must succeed");
    assert!(
        extract_structured(&searched).get("tools").is_some(),
        "catalog.search must return a real result, not an empty stand-in: {searched}"
    );

    // Then: "as the new tenant" -- the same key-less session resolves to the
    // tenant its own `signup` created, not to some other tenant and not to
    // an admin.
    let whoami = session
        .tools_call("host.whoami", json!({}))
        .await
        .expect("a key-less host.whoami on the bound session must succeed");
    assert_eq!(
        extract_structured(&whoami)["tenant"].as_str(),
        Some(namespace.as_str()),
        "the key-less call must run as the tenant this session created: {whoami}"
    );

    // Then: "no response on that session carries error_code
    // tenant_key_missing" -- re-run the whole key-less surface once more and
    // assert none of it refuses.
    for tool in ["host.catalog.search", "host.whoami", "host.usage"] {
        let result = session.tools_call(tool, json!({})).await;
        let err = result.err();
        assert!(
            err.is_none(),
            "no key-less call on a bound session may be refused ({tool}): {err:?}"
        );
    }
}

/// The binding is addressed by a session id this process minted, never by
/// client input (the PRD's own non-functional clause). A caller that invents
/// its own `Mcp-Session-Id` -- or replays a real one it guessed a prefix of
/// -- is handed a fresh session instead and stays anonymous.
#[tokio::test]
async fn a_client_invented_session_id_never_addresses_a_binding() {
    let server = TestServer::start().await;
    let session = McpClient::new(&server.base_url).with_session_continuity();
    let signed_up = session
        .tools_call("signup", json!({"name": "AC1 Forgery Tenant"}))
        .await
        .expect("signup");
    let real_tenant = extract_structured(&signed_up)["tenant"]
        .as_str()
        .expect("tenant namespace")
        .to_string();
    let real = session.session_id().expect("the server issued a session id");

    let http = reqwest::Client::new();
    for forged in [
        "made-up-session-id".to_string(),
        format!("{}.{}", "0".repeat(32), "0".repeat(32)),
        // The real id's own nonce half with a wrong tag.
        format!("{}.{}", real.split('.').next().unwrap(), "f".repeat(32)),
    ] {
        let response = http
            .post(format!("{}/mcp", server.base_url))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2026-07-28")
            .header("Mcp-Method", "tools/call")
            .header("Mcp-Name", "host.catalog.search")
            .header("Mcp-Session-Id", &forged)
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "host.catalog.search",
                    "arguments": {},
                    "_meta": {
                        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                        "io.modelcontextprotocol/clientCapabilities": {},
                        "io.modelcontextprotocol/clientInfo": {"name": "forger", "version": "0.1"},
                    },
                },
            }))
            .send()
            .await
            .expect("send forged-session request");
        let issued = response
            .headers()
            .get("Mcp-Session-Id")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        // PRD-mcphost-implicit-signup: a forged session id is still never
        // bound to the real tenant -- `tenant_key_missing` is no longer
        // the proof of that (a bare host.* call on /mcp now implicitly
        // signs up instead of refusing), so the proof is that the call
        // succeeds as some OTHER, freshly-minted tenant, never
        // "AC1 Forgery Tenant". That implicit signup binds the session and
        // upgrades the response to text/event-stream -- see
        // parse_response_body's own doc comment.
        let body = parse_response_body(response).await;
        assert!(body.get("error").is_none(), "must not error: {body:?}");
        let structured = body["result"]
            .get("structuredContent")
            .cloned()
            .unwrap_or_else(|| body["result"].clone());
        if let Some(onboarding) = structured.get("onboarding") {
            assert_ne!(
                onboarding["tenant"].as_str(),
                Some(real_tenant.as_str()),
                "a client-chosen session id must never reach the real tenant's binding ({forged}): {body}"
            );
        }
        assert_ne!(
            issued.as_deref(),
            Some(forged.as_str()),
            "the server must replace a value it never minted with a fresh one, not echo it back"
        );
    }
}

/// A raw HTTP client deliberately independent of `McpClient` -- mirrors
/// `compat_ac11_ac12_claude_sdk_replay.rs`'s own `post` helper exactly,
/// since this test replays the identical sequence against a differently-
/// configured router.
async fn post(
    http: &reqwest::Client,
    base_url: &str,
    body: &Value,
    protocol_version_header: Option<&str>,
) -> reqwest::Response {
    let mut req = http
        .post(format!("{base_url}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(body);
    if let Some(v) = protocol_version_header {
        req = req.header("MCP-Protocol-Version", v);
    }
    req.send().await.expect("send request")
}

/// Why the session id is minted by mcphost rather than by `rmcp`: builds the
/// exact production router (`build_router_with_session_mode`) with
/// `legacy_session_mode(true)` -- `rmcp` 3.3.0's only built-in session-id
/// mechanism -- and replays the Claude Agent SDK wire sequence
/// `compat_ac11_ac12_claude_sdk_replay.rs::ac12` proves succeeds today. The
/// SDK never echoes `Mcp-Session-Id`, so `rmcp` reads its second request as
/// an attempted `initialize` and rejects it with HTTP 422 before any handler
/// sees it. `build_router` therefore still passes `false`, and
/// `sessbind_ac01`'s first test proves the binding works anyway.
#[tokio::test]
async fn enabling_rmcps_own_session_mode_would_break_claude_sdk_replay() {
    let (state, _data_dir) = common::bare_app_state().await;
    let state = Arc::new(state);

    let listener = tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local addr");
    let base_url = format!("http://{addr}");

    let app = mcphost::http::build_router_with_session_mode(state, true);
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let http = reqwest::Client::new();

    let init_resp = post(
        &http,
        &base_url,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "claude-agent-sdk", "version": "0.1"}
            }
        }),
        None,
    )
    .await;
    assert!(
        init_resp.status().is_success(),
        "initialize must still succeed with legacy_session_mode(true): {}",
        init_resp.status()
    );
    let negotiated = init_resp
        .headers()
        .get("MCP-Protocol-Version")
        .and_then(|v| v.to_str().ok())
        .expect("initialize response carries MCP-Protocol-Version")
        .to_string();

    // tools/list, carrying the negotiated version, no Mcp-Session-Id --
    // exactly what the real Claude Agent SDK sends today.
    let list_resp = post(
        &http,
        &base_url,
        &json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {},
        }),
        Some(&negotiated),
    )
    .await;

    assert_eq!(
        list_resp.status(),
        reqwest::StatusCode::UNPROCESSABLE_ENTITY,
        "with legacy_session_mode(true), a header-less follow-up request from the real \
         Claude Agent SDK's own recorded sequence must be rejected (rmcp reads it as an \
         attempted second `initialize`) -- this is why mcphost mints the session id \
         itself instead: {}",
        list_resp.status()
    );
}
