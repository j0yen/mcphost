//! PRD-mcphost-connect-start-and-callback
//! AC4 (P0) -- Given a redeemed handoff token, When `/connections/go/<token>`
//! is opened again, Then it returns 410 and no second authorization starts;
//! When the callback is called with that `state`, Then it returns 400
//! `state_invalid`.

use crate::common;
use common::testoauth::TestOAuth;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn cstart_ac04() {
    let oauth = TestOAuth::start().await;
    let server = TestServer::start_with_connect_providers(oauth.connect_providers()).await;
    let (_ns, key) = signup(&server.base_url, "CStart AC4").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let url = extract_structured(&client.tools_call("host.connect.start", json!({"provider": "testprov"})).await.unwrap())
        ["url"]
        .as_str()
        .unwrap()
        .to_string();

    // A callback before the link was ever opened carries a state nobody redeemed.
    let token = url.rsplit('/').next().unwrap();
    let early = reqwest::get(format!("{}/connections/callback/testprov?code=x&state={token}", server.base_url))
        .await
        .unwrap();
    assert_eq!(early.status(), reqwest::StatusCode::BAD_REQUEST);
    assert!(early.text().await.unwrap().contains("state_invalid"));

    // Redeem it (the browser follows the link and the callback completes).
    let callback_url = oauth.follow_to_callback(&url).await;
    let done = reqwest::get(&callback_url).await.unwrap();
    assert_eq!(done.status(), reqwest::StatusCode::OK);
    assert_eq!(oauth.authorize_requests().len(), 1);

    // Opening the same link again: 410, and no second authorization starts.
    let no_redirect = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let again = no_redirect.get(&url).send().await.unwrap();
    assert_eq!(again.status(), reqwest::StatusCode::GONE);
    assert!(again.headers().get(reqwest::header::LOCATION).is_none(), "a used link redirects nowhere");
    assert_eq!(oauth.authorize_requests().len(), 1, "no second authorization started");

    // The callback replayed with the same state: 400 state_invalid, no second token exchange.
    let exchanges = oauth.token_calls();
    let replay = reqwest::get(&callback_url).await.unwrap();
    assert_eq!(replay.status(), reqwest::StatusCode::BAD_REQUEST);
    assert!(replay.text().await.unwrap().contains("state_invalid"));
    assert_eq!(oauth.token_calls(), exchanges, "a used state exchanges nothing");

    // An unknown state and a wrong-provider state are state_invalid too.
    for bad in [
        format!("{}/connections/callback/testprov?code=x&state=bogus", server.base_url),
        format!("{}/connections/callback/github?code=x&state={token}", server.base_url),
        format!("{}/connections/callback/testprov?code=x", server.base_url),
    ] {
        let r = reqwest::get(&bad).await.unwrap();
        assert_eq!(r.status(), reqwest::StatusCode::BAD_REQUEST, "{bad}");
        assert!(r.text().await.unwrap().contains("state_invalid"), "{bad}");
    }
}
