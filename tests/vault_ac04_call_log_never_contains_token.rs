//! PRD-mcphost-upstream-token-vault
//! AC4 (P0) — Given the same call, When `host.runs.get` is read, Then the
//! run log contains no substring of the access or refresh token.
//!
//! `host.runs.get` reads a `runs` row, which this build only ever creates
//! for an async job dispatch (`host.tool_call {async: true}` /
//! `runs::enqueue`); a job's own `CallCtx` carries no end-user identity
//! (`runs.rs`'s own doc comment: "an async/scheduled job has no live
//! request to carry an end user from"), so a vault-injected call is always
//! the ordinary synchronous `tools/call` path this suite's other vault ACs
//! exercise, not a `runs` row. This test instead reads back
//! `host.tool_logs` -- the persisted per-call log line `ctx.log` writes for
//! exactly this kind of call (`handler.rs`'s own doc comment: "flushed into
//! `host.tool_logs`") -- and proves the vault token (deliberately pushed
//! into `kinds::http`'s redaction list, same as any `secret.<name>` value)
//! never reaches it.
//!
//! The PRD's own Technical considerations spell out the real threat this
//! guards against: "the response path strips nothing (tokens do not
//! appear in responses unless the provider echoes them, which the
//! redaction filter for `Authorization`-shaped strings in run logs
//! handles)". `kinds::http`'s only two `ctx.log.log` calls are a bare
//! `{method} {host} {status} {duration}ms` line with no header/body
//! interpolation on the *success* path, so a call that never errors can
//! never exercise the redaction this AC is about -- the log line
//! structurally excludes the token regardless of whether the vault-token
//! redaction registration exists. To make the vault token's actual
//! reachable path meaningful, this test drives the upstream to a 401 that
//! echoes the token back in its error body (exactly the "provider echoes
//! them" case above): `kinds::http`'s error branch folds a redacted
//! excerpt of that body into the same persisted log line, so removing the
//! `secret_values.push(token.clone())` registration at the injection site
//! would let the raw token reach `host.tool_logs` here, and this test
//! would fail.

use crate::common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::Serialize;
use serde_json::json;
use wiremock::matchers::{header, method};
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

async fn sign_assertion(client: &McpClient, subject: &str) -> String {
    let rotate = client
        .tools_call("host.enduser.assertion_secret_rotate", json!({}))
        .await
        .expect("assertion_secret_rotate ok");
    let secret = extract_structured(&rotate)["secret"]
        .as_str()
        .expect("secret string")
        .to_string();
    let now = now_unix();
    let claims = AssertionClaims { sub: subject.to_string(), iat: now, exp: now + 300 };
    encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .expect("sign assertion")
}

#[tokio::test]
async fn tool_logs_never_carry_the_vault_token() {
    const VAULT_TOKEN: &str = "u1-vault-access-token-for-ac4";

    let upstream = MockServer::start().await;
    // The upstream deliberately echoes the vault token back in a 401 body
    // (the PRD's "unless the provider echoes them" case) -- a real
    // exercise of the redaction, not a status line that could never carry
    // it either way.
    Mock::given(method("GET"))
        .and(header("Authorization", format!("Bearer {VAULT_TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": "invalid_token",
            "token_used": VAULT_TOKEN,
        })))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns.clone())
        .await
        .unwrap()
        .expect("tenant");
    let (access_enc, access_nonce) = server.state.secrets.encrypt(VAULT_TOKEN).expect("encrypt");
    server
        .state
        .db
        .upsert_vault_token(
            tenant.id,
            "slack".to_string(),
            "u1".to_string(),
            None,
            access_enc,
            access_nonce,
            None,
            None,
            now_unix() + 3600,
            "read".to_string(),
        )
        .await
        .expect("seed vault token");

    let spec = json!({
        "method": "GET",
        "url": format!("{}/whoami", upstream.uri()),
        "upstream_provider": "slack",
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "slack_call", "kind": "http", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let assertion = sign_assertion(&client, "u1").await;
    let err = client
        .tools_call(
            &format!("{ns}.slack_call"),
            json!({"end_user_assertion": assertion}),
        )
        .await
        .expect_err("upstream 401 must surface as a call error");
    assert_eq!(err.error_code.as_deref(), Some("upstream_status"));
    // The error's own structured data proves the upstream really did echo
    // the token back, and that the existing redaction already scrubs it
    // from the surface the caller sees directly.
    let err_dump = err.data.to_string();
    assert!(!err_dump.contains(VAULT_TOKEN), "error data must never carry the vault token: {err_dump}");

    let logs = client
        .tools_call("host.tool_logs", json!({"name": "slack_call"}))
        .await
        .expect("tool_logs ok");
    let lines = extract_structured(&logs)["lines"].clone();
    let joined = lines.to_string();
    assert!(!joined.contains(VAULT_TOKEN), "tool_logs must never carry the vault token: {joined}");
    assert!(
        joined.contains("body="),
        "expected the error branch's redacted excerpt to reach the persisted log: {joined}"
    );
}
