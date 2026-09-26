//! PRD-mcphost-runs-end-user-subject
//! AC2 (P0) — Given calls for `alice` (2 runs) and `bob` (1 run), When
//! `host.runs.list {end_user_subject: "alice"}` runs, Then exactly the 2
//! alice runs return; When `{end_user_subject: "carol"}`, Then an empty
//! list.

use crate::common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::Serialize;
use serde_json::json;
use std::time::{Duration, Instant};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[derive(Serialize)]
struct AssertionClaims {
    sub: String,
    iat: i64,
    exp: i64,
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn sign_assertion(secret: &str, subject: &str) -> String {
    let now = now_unix();
    let claims = AssertionClaims { sub: subject.to_string(), iat: now, exp: now + 300 };
    encode(&Header::new(Algorithm::HS256), &claims, &EncodingKey::from_secret(secret.as_bytes()))
        .expect("sign assertion")
}

async fn enqueue_for(client: &McpClient, secret: &str, subject: &str) -> String {
    let assertion = sign_assertion(secret, subject);
    let enqueue = extract_structured(
        &client
            .tools_call(
                "host.tool_call",
                json!({
                    "name": "ping_tool",
                    "args": {},
                    "async": true,
                    "end_user_assertion": assertion,
                }),
            )
            .await
            .expect("enqueue ok"),
    );
    enqueue["run_id"].as_str().expect("run_id").to_string()
}

async fn wait_done(client: &McpClient, run_id: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let got = extract_structured(
            &client
                .tools_call("host.runs.get", json!({"run_id": run_id}))
                .await
                .expect("runs.get ok"),
        );
        if got["status"] == json!("done") {
            return;
        }
        if Instant::now() >= deadline {
            panic!("run {run_id} never reached done within 10s: {got}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn list_filters_runs_by_end_user_subject() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ping"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let rotate = client
        .tools_call("host.enduser.assertion_secret_rotate", json!({}))
        .await
        .expect("assertion_secret_rotate ok");
    let secret = extract_structured(&rotate)["secret"]
        .as_str()
        .expect("secret string")
        .to_string();

    let spec = json!({
        "method": "GET",
        "url": format!("{}/ping", upstream.uri()),
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "ping_tool", "kind": "http", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let alice_run_1 = enqueue_for(&client, &secret, "alice").await;
    let alice_run_2 = enqueue_for(&client, &secret, "alice").await;
    let bob_run = enqueue_for(&client, &secret, "bob").await;
    wait_done(&client, &alice_run_1).await;
    wait_done(&client, &alice_run_2).await;
    wait_done(&client, &bob_run).await;

    let alice_list = extract_structured(
        &client
            .tools_call("host.runs.list", json!({"end_user_subject": "alice"}))
            .await
            .expect("runs.list alice ok"),
    );
    let alice_runs = alice_list["runs"].as_array().expect("runs array");
    assert_eq!(alice_runs.len(), 2, "expected exactly 2 alice runs: {alice_list}");
    let alice_ids: Vec<&str> = alice_runs.iter().map(|r| r["run_id"].as_str().unwrap()).collect();
    assert!(alice_ids.contains(&alice_run_1.as_str()), "{alice_list}");
    assert!(alice_ids.contains(&alice_run_2.as_str()), "{alice_list}");
    for r in alice_runs {
        assert_eq!(r["end_user"]["subject"], json!("alice"), "{r}");
    }

    let carol_list = extract_structured(
        &client
            .tools_call("host.runs.list", json!({"end_user_subject": "carol"}))
            .await
            .expect("runs.list carol ok"),
    );
    let carol_runs = carol_list["runs"].as_array().expect("runs array");
    assert!(carol_runs.is_empty(), "unknown subject must return an empty list: {carol_list}");
}
