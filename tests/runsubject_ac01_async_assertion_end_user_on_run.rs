//! PRD-mcphost-runs-end-user-subject
//! AC1 (P0) — Given a tenant with an assertion secret, When a tool is
//! called async with an `end_user_assertion` for subject `alice`, Then
//! `host.runs.get {id}` returns `end_user: {subject: "alice", issuer: null,
//! method: "assertion"}`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::Serialize;
use serde_json::{Value, json};
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

#[tokio::test]
async fn async_call_with_assertion_carries_end_user_onto_the_run() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ping"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "AC1 Tenant").await;
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

    let now = now_unix();
    let claims = AssertionClaims { sub: "alice".to_string(), iat: now, exp: now + 300 };
    let assertion = encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .expect("sign assertion");

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
    assert_eq!(enqueue["status"], json!("queued"));
    let run_id = enqueue["run_id"].as_str().expect("run_id").to_string();

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let got = extract_structured(
            &client
                .tools_call("host.runs.get", json!({"run_id": run_id}))
                .await
                .expect("runs.get ok"),
        );
        if got["status"] == json!("done") {
            assert_eq!(
                got["end_user"],
                json!({"subject": "alice", "issuer": Value::Null, "method": "assertion"}),
                "run's end_user: {got}"
            );
            return;
        }
        if Instant::now() >= deadline {
            panic!("run never reached done within 10s: {got}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
