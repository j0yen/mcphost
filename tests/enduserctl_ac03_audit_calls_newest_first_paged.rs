//! PRD-mcphost-end-user-audit-and-revoke
//! AC3 (P0) -- Given u1 made calls to tools A and B, When
//! `host.enduser.audit {subject: "u1"}` runs, Then both calls return with
//! tool, outcome, run id, and `impersonated: false`, newest first, paged
//! by cursor.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
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

fn sign_assertion(secret: &str, sub: &str) -> String {
    let now = now_unix();
    let claims = AssertionClaims { sub: sub.to_string(), iat: now, exp: now + 300 };
    encode(&Header::new(Algorithm::HS256), &claims, &EncodingKey::from_secret(secret.as_bytes()))
        .expect("sign assertion")
}

#[tokio::test]
async fn audit_returns_both_calls_newest_first_paged() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    for name in ["toola", "toolb"] {
        client
            .tools_call(
                "host.tool_publish",
                json!({"name": name, "kind": "echo", "spec": {"schema": {"type": "object"}}}),
            )
            .await
            .unwrap_or_else(|e| panic!("publish {name} failed: {} {}", e.code, e.message));
    }

    let rotate = client
        .tools_call("host.enduser.assertion_secret_rotate", json!({}))
        .await
        .expect("assertion_secret_rotate ok");
    let secret = extract_structured(&rotate)["secret"].as_str().expect("secret string").to_string();

    // AC3's "made calls to tools A and B" -- toola first, then toolb, so
    // toolb is the newer of the two.
    for name in ["toola", "toolb"] {
        let assertion = sign_assertion(&secret, "u1");
        client
            .tools_call(&format!("{ns}.{name}"), json!({"end_user_assertion": assertion}))
            .await
            .unwrap_or_else(|e| panic!("u1 call to {name} must succeed: {} {}", e.code, e.message));
    }

    let audited = extract_structured(
        &client
            .tools_call("host.enduser.audit", json!({"subject": "u1"}))
            .await
            .expect("host.enduser.audit ok"),
    );
    let entries = audited["entries"].as_array().expect("entries array");
    assert_eq!(entries.len(), 2, "entries: {entries:?}");

    // newest first: toolb (called second) before toola.
    assert_eq!(entries[0]["type"], json!("call"), "{:?}", entries[0]);
    assert_eq!(entries[0]["tool"], json!("toolb"), "{:?}", entries[0]);
    assert_eq!(entries[0]["outcome"], json!("ok"), "{:?}", entries[0]);
    assert_eq!(entries[0]["impersonated"], json!(false), "{:?}", entries[0]);
    assert!(entries[0]["run_id"].as_str().is_some(), "{:?}", entries[0]);

    assert_eq!(entries[1]["tool"], json!("toola"), "{:?}", entries[1]);
    assert_eq!(entries[1]["outcome"], json!("ok"), "{:?}", entries[1]);
    assert_eq!(entries[1]["impersonated"], json!(false), "{:?}", entries[1]);
    assert!(entries[1]["run_id"].as_str().is_some(), "{:?}", entries[1]);
    assert_ne!(entries[0]["run_id"], entries[1]["run_id"], "each call gets its own run id");

    // paged by cursor: one at a time covers the same two entries in order.
    let page1 = extract_structured(
        &client
            .tools_call("host.enduser.audit", json!({"subject": "u1", "limit": 1}))
            .await
            .expect("page1 ok"),
    );
    let page1_entries = page1["entries"].as_array().expect("page1 entries");
    assert_eq!(page1_entries.len(), 1, "{page1_entries:?}");
    assert_eq!(page1_entries[0]["tool"], json!("toolb"), "{page1_entries:?}");
    let cursor = page1["cursor"].as_str().expect("cursor present for page1").to_string();

    let page2 = extract_structured(
        &client
            .tools_call("host.enduser.audit", json!({"subject": "u1", "limit": 1, "cursor": cursor}))
            .await
            .expect("page2 ok"),
    );
    let page2_entries = page2["entries"].as_array().expect("page2 entries");
    assert_eq!(page2_entries.len(), 1, "{page2_entries:?}");
    assert_eq!(page2_entries[0]["tool"], json!("toola"), "{page2_entries:?}");
    assert_eq!(page2["cursor"], json!(null), "no more pages: {page2:?}");
}
