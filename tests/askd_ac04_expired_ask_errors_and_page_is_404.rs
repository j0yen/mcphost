//! PRD-mcphost-ask-user-decision
//! AC4 -- Given an ask with `ttl_s: 2`, When `host.ask.wait` is called 3 s
//! later, Then it returns error `ask_expired` with `next_call:
//! host.ask.create`, and the page returns the 404 bytes.

use crate::common;

use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn wait_after_ttl_is_ask_expired_and_the_page_is_the_router_404() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "askd ac04 tenant").await;
    let agent = McpClient::with_bearer(&server.base_url, &key);

    let created = extract_structured(
        &agent
            .tools_call("host.ask.create", json!({"question": "Send it?", "options": ["yes", "no"], "ttl_s": 2}))
            .await
            .expect("ask.create"),
    );
    let ask_id = created["ask_id"].as_str().expect("ask_id").to_string();
    let advertised = created["url"].as_str().expect("url");
    let url = format!("{}/a/{}", server.base_url, advertised.split_once("/a/").expect("/a/ url").1);

    let http = reqwest::Client::new();
    assert_eq!(http.get(&url).send().await.expect("live page").status(), 200, "answerable before the ttl");

    tokio::time::sleep(Duration::from_secs(3)).await;

    let err = agent
        .tools_call("host.ask.wait", json!({"ask_id": ask_id, "timeout_s": 1}))
        .await
        .expect_err("a lapsed ask must fail, not return pending");
    assert_eq!(err.error_code.as_deref(), Some("ask_expired"), "{err:?}");
    assert_eq!(err.data["next_call"], json!("host.ask.create"), "{err:?}");

    // The page is the router's own no-route 404: same status, same bytes.
    let page = http.get(&url).send().await.expect("expired page");
    let nowhere = http.get(format!("{}/no-such-route-askd", server.base_url)).send().await.expect("fallback");
    assert_eq!(page.status(), 404);
    assert_eq!(nowhere.status(), 404);
    assert_eq!(page.bytes().await.expect("page bytes"), nowhere.bytes().await.expect("fallback bytes"));

    let posted = http.post(&url).form(&[("choice", "yes")]).send().await.expect("late post");
    assert_eq!(posted.status(), 404, "an expired link cannot be answered either");
}
