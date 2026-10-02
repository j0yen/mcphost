//! PRD-mcphost-chain-run-lineage AC7 (P0) — Given a 3-step chain run by an
//! end user with `end_user_subject = "alice"`, When children are read,
//! Then each child's `end_user` equals the parent's.

use crate::common;
use common::{chain_kind_registry, extract_structured, publish, signup, McpClient, TestServer};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde::Serialize;
use serde_json::json;

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
async fn children_carry_the_same_end_user_as_their_parent() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "AC7 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let rotate = client
        .tools_call("host.enduser.assertion_secret_rotate", json!({}))
        .await
        .expect("assertion_secret_rotate ok");
    let secret = extract_structured(&rotate)["secret"]
        .as_str()
        .expect("secret string")
        .to_string();

    let schema = json!({"type": "object"});
    publish(&client, "fetch_data", "echo", json!({"schema": schema})).await;
    publish(&client, "transform", "echo", json!({"schema": schema})).await;
    publish(&client, "write", "echo", json!({"schema": schema})).await;

    let chain_spec = json!({
        "steps": [
            {"tool": "fetch_data", "args": {"url": "$.input.url"}},
            {"tool": "transform", "args": {"rows": "$.prev.result.url"}},
            {"tool": "write", "args": {"rows": "$.prev.result.rows", "region": "$.input.region"}},
        ]
    });
    let chain = publish(&client, "daily_pipeline", "chain", chain_spec).await;
    assert_eq!(chain, format!("{ns}.daily_pipeline"));

    let now = now_unix();
    let claims = AssertionClaims { sub: "alice".to_string(), iat: now, exp: now + 300 };
    let assertion = encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .expect("sign assertion");

    client
        .tools_call(
            "host.tool_call",
            json!({
                "name": chain,
                "args": {"url": "https://example.com/data", "region": "eu"},
                "end_user_assertion": assertion,
            }),
        )
        .await
        .expect("call ok");

    let runs = extract_structured(
        &client
            .tools_call("host.runs.list", json!({}))
            .await
            .expect("runs.list ok"),
    );
    let parent = &runs["runs"].as_array().expect("runs array")[0];
    assert_eq!(
        parent["end_user"]["subject"], json!("alice"),
        "the parent itself must carry the end user: {parent}"
    );
    let parent_id = parent["run_id"].as_str().expect("parent run_id").to_string();

    let got = extract_structured(
        &client
            .tools_call("host.runs.get", json!({"run_id": parent_id}))
            .await
            .expect("runs.get ok"),
    );
    let children = got["children"].as_array().expect("children array");
    assert_eq!(children.len(), 3);
    for child in children {
        assert_eq!(
            child["end_user"], parent["end_user"],
            "every child's end_user must equal the parent's: {child}"
        );
    }
}
