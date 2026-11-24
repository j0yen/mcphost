//! PRD-mcphost-implicit-signup-key-in-hand
//! AC5 (P0) -- Given a key-bearing call from the recent peer, a recent tenant
//! that is disabled, a presented different session, and a mock clock past
//! 600 s, When a key-less `host.table.create` follows each, Then the
//! `frees_peer` and `TenantDisabled` branches answer as at v0.120.0, the
//! presented session's yield fires without counting toward the cap, and the
//! elapsed window resets `mints_in_window` to 0.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured};
use mcphost::peer_recent::{PEER_TTL_SECS, peer_key};
use serde_json::{Value, json};

fn create_args() -> Value {
    json!({"name": "runs", "columns": {"id": "integer"}})
}

/// A sessionless first call: returns the onboarding object.
async fn sessionless_mint(server: &TestServer, ua: &str) -> Value {
    let created = extract_structured(
        &McpClient::new(&server.base_url)
            .with_user_agent(ua)
            .tools_call("host.table.create", create_args())
            .await
            .expect("a sessionless key-less call mints"),
    );
    created["onboarding"].clone()
}

fn mints_in_window(server: &TestServer, ua: &str) -> usize {
    let peer = peer_key("127.0.0.1", Some(ua));
    server.state.redeemed_peers.state(&peer, mcphost::state::now_unix()).map_or(0, |s| s.mints_in_window)
}

#[tokio::test]
async fn kih_ac05() {
    let server = TestServer::start_with_signup_rate_limit(1000).await;
    server.state.redeemed_peers.set_cap(3);
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    // (1) A key-bearing call from the recent peer frees it: the next key-less
    // call is an implicit signup again, counted from 0.
    let ua = "kih-ac05/key-bearing";
    let first = sessionless_mint(&server, ua).await;
    assert_eq!(mints_in_window(&server, ua), 1);
    McpClient::new(&server.base_url)
        .with_user_agent(ua)
        .tools_call("host.table.list", json!({"tenant_key": first["tenant_key"]}))
        .await
        .expect("the handed key works");
    assert_eq!(mints_in_window(&server, ua), 0, "the peer is freed");
    let again = sessionless_mint(&server, ua).await;
    assert_ne!(again["tenant"], first["tenant"], "a fresh implicit signup");
    assert_eq!(mints_in_window(&server, ua), 1);

    // (1b) The frees_peer branch itself: the recent tenant self-offboards over
    // its URL (no tenant_key argument, so the record stays), and the next
    // key-less call is an implicit signup as at v0.120.0.
    let ua = "kih-ac05/frees-peer";
    let minted = sessionless_mint(&server, ua).await;
    let url = minted["url"].as_str().expect("onboarding.url");
    let path = url.strip_prefix(&server.base_url).expect("url on this server").to_string();
    McpClient::new(&server.base_url)
        .with_path(&path)
        .tools_call("host.self.offboard", json!({}))
        .await
        .expect("self-offboard on its URL");
    assert_eq!(mints_in_window(&server, ua), 1, "the record still holds the offboarded tenant");
    let after = sessionless_mint(&server, ua).await;
    assert_ne!(after["tenant"], minted["tenant"], "frees_peer: a new tenant, never a refusal");

    // (2) A recent tenant that is disabled answers tenant_disabled once, then
    // the peer is free.
    let ua = "kih-ac05/disabled";
    let minted = sessionless_mint(&server, ua).await;
    admin
        .tools_call("admin.tenant_disable", json!({"tenant": minted["tenant"]}))
        .await
        .expect("disable");
    let err = McpClient::new(&server.base_url)
        .with_user_agent(ua)
        .tools_call("host.table.create", create_args())
        .await
        .expect_err("a disabled recent tenant is named, not re-signed-up");
    assert_eq!(err.error_code.as_deref(), Some("tenant_disabled"), "{err:?}");
    let next = sessionless_mint(&server, ua).await;
    assert_ne!(next["tenant"], minted["tenant"], "the TenantDisabled branch forgot the peer");

    // (3) A presented different session yields and does not count toward the cap.
    let ua = "kih-ac05/presented";
    sessionless_mint(&server, ua).await;
    assert_eq!(mints_in_window(&server, ua), 1);
    for n in 0..5 {
        let session = McpClient::new(&server.base_url).with_user_agent(ua).with_session_continuity();
        session.initialize().await;
        let created = extract_structured(
            &session.tools_call("host.table.create", create_args()).await.unwrap_or_else(|e| panic!("session {n} yields: {e:?}")),
        );
        assert!(created["onboarding"]["tenant"].is_string(), "session {n}: {created}");
    }
    assert_eq!(mints_in_window(&server, ua), 1, "five presented-session yields counted nothing");
    sessionless_mint(&server, ua).await;
    sessionless_mint(&server, ua).await;
    assert_eq!(mints_in_window(&server, ua), 3, "only sessionless mints count");
    McpClient::new(&server.base_url)
        .with_user_agent(ua)
        .tools_call("host.table.create", create_args())
        .await
        .expect_err("the cap still binds a sessionless call");

    // (4) The window elapsed (mock clock): the count is 0 and a key-less call mints.
    let ua = "kih-ac05/window";
    let minted = sessionless_mint(&server, ua).await;
    let peer = peer_key("127.0.0.1", Some(ua));
    let namespace = minted["tenant"].as_str().expect("tenant");
    let now = mcphost::state::now_unix();
    server.state.redeemed_peers.forget(&peer);
    for _ in 0..3 {
        server.state.redeemed_peers.record_sessionless_mint_at(&peer, namespace, None, now - 30);
    }
    let held = McpClient::new(&server.base_url)
        .with_user_agent(ua)
        .tools_call("host.table.create", create_args())
        .await
        .expect_err("three mints 30 s ago are at the cap");
    assert_eq!(held.data["mints_in_window"], json!(3), "{held:?}");
    server.state.redeemed_peers.forget(&peer);
    for _ in 0..3 {
        server.state.redeemed_peers.record_sessionless_mint_at(&peer, namespace, None, now - PEER_TTL_SECS - 1);
    }
    assert_eq!(mints_in_window(&server, ua), 0, "the elapsed window resets the count to 0");
    let reset = sessionless_mint(&server, ua).await;
    assert_ne!(reset["tenant"], minted["tenant"], "a key-less call past the window mints");
    assert_eq!(mints_in_window(&server, ua), 1);
}
