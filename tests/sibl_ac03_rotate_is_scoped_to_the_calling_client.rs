//! PRD-mcphost-same-user-second-agent
//! AC3 — Given two siblings, When client 0 calls `host.key.rotate`, Then
//! client 0's old URL answers `tenant_key_invalid`, the sibling's URL still
//! works, and the join code is unchanged; with `all: true` all three change.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

async fn whoami(base: &str, path: &str) -> Result<Value, common::RpcError> {
    McpClient::new(base)
        .with_path(path)
        .tools_call("host.whoami", json!({}))
        .await
        .map(|r| extract_structured(&r))
}

fn path_of(base: &str, url: &str) -> String {
    url.trim_start_matches(base).to_string()
}

#[tokio::test]
async fn sibl_ac03_rotate_is_scoped_to_the_calling_client() {
    let server = TestServer::start().await;
    let base = server.base_url.clone();
    let (ns, key) = signup(&base, "Rotating Host").await;

    // Client 0 gets a URL of its own (rotating over the key mints the first one).
    let minted = extract_structured(
        &McpClient::with_bearer(&base, &key).tools_call("host.key.rotate", json!({})).await.expect("first rotate"),
    );
    let mut zero_path = path_of(&base, minted["url"].as_str().expect("client 0 url"));
    let me = whoami(&base, &zero_path).await.expect("client 0 whoami");
    let join_url = me["join_url"].as_str().expect("join_url").to_string();
    let join_path = path_of(&base, &join_url);

    // Two siblings join.
    let mut sibling_paths = Vec::new();
    for _ in 0..2 {
        let joiner = McpClient::new(&base).with_path(&join_path).with_session_continuity();
        let joined = extract_structured(&joiner.tools_call("host.whoami", json!({})).await.expect("join"));
        assert_eq!(joined["namespace"], json!(ns));
        sibling_paths.push(path_of(&base, joined["onboarding"]["url"].as_str().expect("sibling url")));
    }

    // Client 0 rotates: only its own URL (and key) change.
    let old_zero = zero_path.clone();
    let rotated = extract_structured(
        &McpClient::new(&base).with_path(&old_zero).tools_call("host.key.rotate", json!({})).await.expect("rotate"),
    );
    zero_path = path_of(&base, rotated["url"].as_str().expect("new client 0 url"));
    let err = whoami(&base, &old_zero).await.expect_err("client 0's old URL must be dead");
    assert_eq!(err.error_code.as_deref(), Some("tenant_key_invalid"), "{err:?}");
    for p in &sibling_paths {
        let s = whoami(&base, p).await.expect("a sibling's URL still works");
        assert_eq!(s["namespace"], json!(ns));
        assert_eq!(s["join_url"], json!(join_url), "the join code is unchanged");
    }
    assert_eq!(whoami(&base, &zero_path).await.expect("new client 0 URL")["join_url"], json!(join_url));

    // `all: true` rotates all three clients and the join code.
    let all = extract_structured(
        &McpClient::new(&base)
            .with_path(&zero_path)
            .tools_call("host.key.rotate", json!({"all": true}))
            .await
            .expect("rotate all"),
    );
    let changed = all["changed"].as_array().expect("the response lists what changed");
    let new_urls: Vec<String> = changed.iter().filter_map(|c| c["url"].as_str()).map(|u| path_of(&base, u)).collect();
    assert_eq!(new_urls.len(), 3, "three client URLs changed: {all}");
    let new_join = all["join_url"].as_str().expect("join_url in the response").to_string();
    assert_ne!(new_join, join_url, "the join code rotated");
    for old in sibling_paths.iter().chain(std::iter::once(&zero_path)) {
        let err = whoami(&base, old).await.expect_err("every old URL is dead after all: true");
        assert_eq!(err.error_code.as_deref(), Some("tenant_key_invalid"), "{old}: {err:?}");
    }
    for fresh in &new_urls {
        let w = whoami(&base, fresh).await.expect("a rotated URL works");
        assert_eq!(w["namespace"], json!(ns));
        assert_eq!(w["join_url"], json!(new_join));
    }
    // The old join link no longer joins anyone: the route 404s before dispatch.
    let stale = reqwest::Client::new()
        .post(format!("{base}{join_path}"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}}))
        .send()
        .await
        .expect("POST the rotated-away join link");
    assert_eq!(stale.status(), reqwest::StatusCode::NOT_FOUND);
}
