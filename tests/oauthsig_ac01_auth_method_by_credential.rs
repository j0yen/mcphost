//! PRD-mcphost-oauth-demand-signal
//! AC1 (P0) — Given calls made with a key, an issuer JWT, and a hosted
//! token, When `calls` is read, Then each row's `auth_method` matches, and
//! rows written before the migration read `key`.
//!
//! Drives the same three credential paths `hostedas_ac10`/`hostedas_ac04`
//! already exercise (key, tenant-registered issuer JWT, this host's own
//! hosted bearer via the full authorize/consent/token exchange), but against
//! a *published tool call* -- `host.whoami`/`host.state.get` never write a
//! `calls` row at all (only `call_published_tool` does), so this test calls
//! the tenant's own published tool three times, once per credential kind,
//! then reads `calls.auth_method` back with a raw connection the same way
//! `provaudit_ac03_migration_backfill_counts.rs` already does for its own
//! pre-migration-row proof.

use crate::common;
use common::{McpClient, TestServer, publish, signup};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::oauth;
use oauth::{KID_1, jwk_1, jwks_server, priv_pem_1, sign};

fn code_challenge_for(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hasher.finalize())
}

fn query_param<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == name).then_some(v)
    })
}

/// Completes the DCR-register -> authorize -> consent -> token exchange
/// (same shape `hostedas_ac04` uses) and returns a hosted bearer access
/// token that runs as `tenant_key`'s tenant.
async fn mint_hosted_bearer(server: &TestServer, tenant_key: &str) -> String {
    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();

    let register: Value = http
        .post(format!("{}/oauth/register", server.base_url))
        .json(&json!({"application_type": "native", "redirect_uris": ["http://127.0.0.1/cb"]}))
        .send()
        .await
        .expect("POST /oauth/register")
        .json()
        .await
        .expect("parse register response");
    let client_id = register["client_id"].as_str().expect("client_id").to_string();

    let verifier = "a-pkce-verifier-at-least-43-chars-long-for-realism";
    let challenge = code_challenge_for(verifier);
    let resource = format!("{}/mcp", server.base_url);

    let consent_resp = http
        .post(format!("{}/oauth/authorize", server.base_url))
        .form(&[
            ("response_type", "code"),
            ("client_id", client_id.as_str()),
            ("redirect_uri", "http://127.0.0.1/cb"),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", "abc"),
            ("scope", "mcp"),
            ("resource", resource.as_str()),
            ("tenant_key", tenant_key),
        ])
        .send()
        .await
        .expect("POST /oauth/authorize");
    assert!(consent_resp.status().is_redirection(), "consent must redirect: {}", consent_resp.status());
    let location = consent_resp.headers().get("location").expect("Location header").to_str().unwrap().to_string();
    let (_redirect_base, query) = location.split_once('?').expect("redirect must carry a query string");
    let code = query_param(query, "code").expect("code present in redirect").to_string();

    let token_resp: Value = http
        .post(format!("{}/oauth/token", server.base_url))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", "http://127.0.0.1/cb"),
            ("client_id", client_id.as_str()),
            ("code_verifier", verifier),
        ])
        .send()
        .await
        .expect("POST /oauth/token")
        .json()
        .await
        .expect("parse token response");
    token_resp["access_token"].as_str().expect("access_token present").to_string()
}

#[tokio::test]
async fn auth_method_matches_the_resolved_credential_and_backfills_key() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "OAuth Demand Signal Tenant").await;
    let key_client = McpClient::with_bearer(&server.base_url, &key);

    let qualified = publish(
        &key_client,
        "echoer",
        "echo",
        json!({"schema": {"type": "object", "properties": {"msg": {"type": "string"}}}}),
    )
    .await;
    assert_eq!(qualified, format!("{ns}.echoer"));

    // 1) key.
    key_client
        .tools_call(&qualified, json!({"msg": "via-key"}))
        .await
        .expect("key-authenticated call must succeed");

    // 2) issuer_jwt: a tenant-registered bring-your-own issuer.
    let jwks = jwks_server(jwk_1()).await;
    let issuer = "https://issuer.example.com";
    let audience = "mcphost-test-audience";
    key_client
        .tools_call(
            "host.oauth.issuer_set",
            json!({"issuer": issuer, "audience": audience, "jwks_url": format!("{}/jwks", jwks.uri())}),
        )
        .await
        .expect("issuer_set must succeed");
    let jwt = sign(KID_1, priv_pem_1(), issuer, audience, "user-1", 300);
    let jwt_client = McpClient::with_bearer(&server.base_url, &jwt);
    jwt_client
        .tools_call(&qualified, json!({"msg": "via-issuer-jwt"}))
        .await
        .expect("issuer-JWT-authenticated call must succeed");

    // 3) hosted_token: this host's own built-in authorization server.
    let hosted_token = mint_hosted_bearer(&server, &key).await;
    let hosted_client = McpClient::with_bearer(&server.base_url, &hosted_token);
    hosted_client
        .tools_call(&qualified, json!({"msg": "via-hosted-token"}))
        .await
        .expect("hosted-token-authenticated call must succeed");

    // Seed one row the way a genuinely pre-migration-0053 row would have
    // looked: an INSERT that never names `auth_method` at all, relying on
    // the column's own `DEFAULT 'key'` -- same pattern
    // `provaudit_ac03_migration_backfill_counts.rs` already uses for its own
    // pre-migration fixture.
    let db_path = server.data_dir.0.join("mcphost.db");
    let tenant_id: i64 = {
        let conn = rusqlite::Connection::open(&db_path).expect("open raw db");
        let tenant_id: i64 = conn
            .query_row("SELECT id FROM tenants WHERE namespace = ?1", [&ns], |r| r.get(0))
            .expect("look up tenant id");
        conn.execute(
            "INSERT INTO calls (tenant_id, tool_name, started_at, started_unix, duration_ms, ok) \
             VALUES (?1, 'legacy_tool', 'unix:0.0', 0, 1, 1)",
            rusqlite::params![tenant_id],
        )
        .expect("seed pre-migration-shaped call row");
        tenant_id
    };

    let conn = rusqlite::Connection::open(&db_path).expect("open raw db");
    let mut stmt = conn
        .prepare(
            "SELECT tool_name, auth_method FROM calls WHERE tenant_id = ?1 ORDER BY id ASC",
        )
        .expect("prepare select");
    let rows: Vec<(String, String)> = stmt
        .query_map([tenant_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("query calls")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect calls rows");

    assert_eq!(
        rows,
        vec![
            ("echoer".to_string(), "key".to_string()),
            ("echoer".to_string(), "issuer_jwt".to_string()),
            ("echoer".to_string(), "hosted_token".to_string()),
            ("legacy_tool".to_string(), "key".to_string()),
        ],
        "each call's auth_method must match its credential, and the pre-migration-shaped row must backfill to 'key'"
    );
}
