//! PRD-mcphost-implicit-signup-key-in-hand
//! AC3 (P0) -- Given `MCPHOST_IMPLICIT_PEER_MINT_CAP=3` and four sessionless
//! key-less `host.table.create` calls from one peer inside the window, When
//! they run, Then the first three mint distinct tenants, each with
//! `onboarding.tenant_key`, and the fourth fails `-32602`
//! `tenant_key_missing` with `data.reason` `peer_recent`,
//! `data.session_presented` `false`, `data.mints_in_window` `3`, `data.cap`
//! `3` and `next_call.retry_after_s` from 1 to 600.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use mcphost::peer_recent::{MINT_CAP_ENV, parse_mint_cap};
use serde_json::{Value, json};

const UA: &str = "kih-ac03/agent";

fn create_args() -> Value {
    json!({"name": "runs", "columns": {"id": "integer"}})
}

#[tokio::test]
async fn kih_ac03() {
    // The env var the startup path reads names the cap the host runs with.
    assert_eq!(MINT_CAP_ENV, "MCPHOST_IMPLICIT_PEER_MINT_CAP");
    assert_eq!(parse_mint_cap(Some("3")), 3);
    let server = TestServer::start_with_signup_rate_limit(1000).await;
    server.state.redeemed_peers.set_cap(parse_mint_cap(Some("3")));

    let mut tenants = vec![];
    for n in 1..=3 {
        let client = McpClient::new(&server.base_url).with_user_agent(UA);
        let created = extract_structured(
            &client
                .tools_call("host.table.create", create_args())
                .await
                .unwrap_or_else(|e| panic!("sessionless mint {n} is inside the cap: {e:?}")),
        );
        assert_eq!(created["implicit"], json!(true), "mint {n}: {created}");
        let onboarding = &created["onboarding"];
        assert!(onboarding["tenant_key"].as_str().is_some_and(|k| !k.is_empty()), "mint {n}: {onboarding}");
        tenants.push(onboarding["tenant"].as_str().expect("onboarding.tenant").to_string());
    }
    let mut distinct = tenants.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(distinct.len(), 3, "three distinct tenants: {tenants:?}");

    let err = McpClient::new(&server.base_url)
        .with_user_agent(UA)
        .tools_call("host.table.create", create_args())
        .await
        .expect_err("the fourth sessionless mint from the peer is over the cap");
    assert_eq!(err.code, -32602, "{err:?}");
    assert_eq!(err.error_code.as_deref(), Some("tenant_key_missing"), "{err:?}");
    assert_eq!(err.data["reason"], json!("peer_recent"), "{err:?}");
    assert_eq!(err.data["session_presented"], json!(false), "{err:?}");
    assert_eq!(err.data["mints_in_window"], json!(3), "{err:?}");
    assert_eq!(err.data["cap"], json!(3), "{err:?}");
    assert_eq!(err.data["window_secs"], json!(600), "{err:?}");
    let retry = err.data["next_call"]["retry_after_s"].as_u64().expect("next_call.retry_after_s");
    assert!((1..=600).contains(&retry), "retry_after_s {retry} is 1..=600: {err:?}");
    assert!(err.data.get("tenant").is_none(), "no tenant named: {err:?}");
    assert_eq!(server.state.db.list_tenants().await.expect("tenants").len(), 3, "the refusal minted nothing");
}

#[tokio::test]
async fn kih_ac03_concurrent_mints_from_one_peer_do_not_pass_the_cap() {
    let server = TestServer::start_with_signup_rate_limit(1000).await;
    server.state.redeemed_peers.set_cap(3);

    let calls = (0..8).map(|_| {
        let client = McpClient::new(&server.base_url).with_user_agent("kih-ac03/concurrent");
        async move { client.tools_call("host.table.create", create_args()).await }
    });
    let results = futures_util::future::join_all(calls).await;
    let minted = results.iter().filter(|r| r.is_ok()).count();
    assert_eq!(minted, 3, "exactly the cap mints under concurrency: {results:?}");
    for refused in results.iter().filter_map(|r| r.as_ref().err()) {
        assert_eq!(refused.data["reason"], json!("peer_recent"), "{refused:?}");
    }
    assert_eq!(server.state.db.list_tenants().await.expect("tenants").len(), 3);
}
