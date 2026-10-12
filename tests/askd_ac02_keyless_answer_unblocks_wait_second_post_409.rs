//! PRD-mcphost-ask-user-decision
//! AC2 -- Given `host.ask.create` with 2 options, When the URL is fetched
//! with no key and one option is POSTed, Then the agent's blocked
//! `host.ask.wait` returns `status: answered` with that choice within 2 s,
//! and a second POST returns 409 `ask_answered` naming the first choice.

use crate::common;

use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};
use std::time::Duration;

/// The ask URL as served by this test host's plain-HTTP listener.
fn local_url(server: &TestServer, advertised: &str) -> String {
    let rest = advertised.split_once("/a/").map(|(_, r)| r).expect("/a/ url");
    format!("{}/a/{rest}", server.base_url)
}

#[tokio::test]
async fn keyless_post_unblocks_the_wait_and_a_second_post_is_409() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "askd ac02 tenant").await;
    let agent = McpClient::with_bearer(&server.base_url, &key);

    let created = extract_structured(
        &agent
            .tools_call("host.ask.create", json!({"question": "Deploy to prod?", "options": ["yes", "no"]}))
            .await
            .expect("ask.create"),
    );
    let ask_id = created["ask_id"].as_str().expect("ask_id").to_string();
    let url = local_url(&server, created["url"].as_str().expect("url"));

    // The agent blocks first.
    let waiter = {
        let (agent, ask_id) = (McpClient::with_bearer(&server.base_url, &key), ask_id.clone());
        tokio::spawn(async move {
            let r = agent
                .tools_call("host.ask.wait", json!({"ask_id": ask_id, "timeout_s": 20}))
                .await
                .expect("ask.wait");
            extract_structured(&r)
        })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;

    // The user: no key, no cookies.
    let http = reqwest::Client::new();
    let page = http.get(&url).send().await.expect("get page");
    assert_eq!(page.status(), 200);
    let html = page.text().await.expect("page body");
    assert!(html.contains("Deploy to prod?"), "{html}");
    assert!(html.contains("value=\"yes\"") && html.contains("value=\"no\""), "{html}");

    let first = http.post(&url).form(&[("choice", "yes")]).send().await.expect("first post");
    assert_eq!(first.status(), 200, "{}", first.text().await.unwrap_or_default());

    // "Within 2 s" is enforced by the timeout itself, not by a measured
    // elapsed time (single-shot timing assertions are flaky).
    let answer = tokio::time::timeout(Duration::from_secs(2), waiter)
        .await
        .expect("the blocked wait must return within 2 s of the answer")
        .expect("waiter joined");
    assert_eq!(answer["status"], json!("answered"), "{answer}");
    assert_eq!(answer["choice"], json!("yes"), "{answer}");
    assert!(answer["answered_unix"].is_i64(), "{answer}");

    let second = http.post(&url).form(&[("choice", "no")]).send().await.expect("second post");
    assert_eq!(second.status(), 409);
    let body: Value = second.json().await.expect("409 json body");
    assert_eq!(body["error"]["code"], json!("ask_answered"), "{body}");
    assert_eq!(body["error"]["choice"], json!("yes"), "{body}");
}
