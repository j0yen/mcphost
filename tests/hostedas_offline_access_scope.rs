//! PRD-mcphost-hosted-authorization-server
//! Live finding (2026-09-27, synthorg OAuth explorer against
//! mcphost.dev): a client registered via `/oauth/register` with
//! `grant_types` `authorization_code` + `refresh_token` -- exactly what
//! every real Claude client is -- requested `scope=mcp offline_access` at
//! `GET /oauth/authorize` against the root resource, and the AS refused it
//! with `invalid_request`, never rendering consent, because the scope
//! check only ever accepted `mcp` verbatim. By the time this branch
//! rebased onto main, PRD-mcphost-tool-scopes-and-consent (#66) had
//! already landed a general per-resource scope catalog
//! (`known_request_scopes`/`resolve_and_validate_scope` in `authz.rs`)
//! that treats `offline_access` as always allowed, order-insensitive,
//! regardless of resource -- so the root-resource case this file pins was
//! already fixed as a side effect, just never exercised by an existing
//! test (every `#66` test authorizes against a per-tenant resource; the
//! `hostedas_ac*` files predate `offline_access` entirely). This AC is
//! the regression test for the exact live repro: root resource, `GET`,
//! `scope=mcp offline_access`.
//!
//! Two things this file deliberately does NOT assert, because `#66`
//! already made and tested the opposite call:
//! * An unknown scope word still redirects with `invalid_request`, not
//!   `invalid_scope` -- `resolve_and_validate_scope`'s own `None` path.
//! * `scopes_supported` does not gain `offline_access` anywhere --
//!   `toolscope_ac07_metadata_scopes_supported`'s own `AC7` already pins
//!   the root document to exactly `["mcp"]`, and `offline_access` is
//!   documented in `known_request_scopes` as deliberately absent from
//!   every catalog ("checked separately, always allowed").

use crate::common;
use common::TestServer;
use serde_json::{Value, json};

const REDIRECT_URI: &str = "http://127.0.0.1/cb";

async fn register_client(server: &TestServer) -> String {
    let http = reqwest::Client::new();
    let register: Value = http
        .post(format!("{}/oauth/register", server.base_url))
        .json(&json!({
            "application_type": "native",
            "redirect_uris": [REDIRECT_URI],
            "grant_types": ["authorization_code", "refresh_token"],
        }))
        .send()
        .await
        .expect("POST /oauth/register")
        .json()
        .await
        .expect("parse register response");
    register["client_id"].as_str().expect("client_id").to_string()
}

async fn authorize_get(server: &TestServer, client_id: &str, scope: &str) -> reqwest::Response {
    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    http.get(format!(
        "{}/oauth/authorize?response_type=code&client_id={client_id}&redirect_uri=http%3A%2F%2F127.0.0.1%2Fcb\
         &code_challenge=dummy&code_challenge_method=S256&state=abc&scope={scope}&resource={}%2Fmcp",
        server.base_url, server.base_url,
    ))
    .send()
    .await
    .expect("GET /oauth/authorize")
}

#[tokio::test]
async fn scope_mcp_offline_access_renders_consent_like_scope_mcp_alone() {
    let server = TestServer::start().await;
    let client_id = register_client(&server).await;

    let baseline = authorize_get(&server, &client_id, "mcp").await;
    assert_eq!(baseline.status(), reqwest::StatusCode::OK, "scope=mcp must render consent");
    assert!(baseline.text().await.unwrap().contains("form"));

    let with_offline = authorize_get(&server, &client_id, "mcp%20offline_access").await;
    assert_eq!(
        with_offline.status(),
        reqwest::StatusCode::OK,
        "scope=mcp offline_access against the root resource must render consent exactly like scope=mcp -- \
         this is the live repro: a real Claude client always requests offline_access alongside mcp"
    );
    assert!(with_offline.headers().get("location").is_none(), "must not redirect anywhere");
    assert!(with_offline.text().await.unwrap().contains("form"));

    // Order-insensitive per RFC 6749: `offline_access mcp` is the same request.
    let reordered = authorize_get(&server, &client_id, "offline_access%20mcp").await;
    assert_eq!(reordered.status(), reqwest::StatusCode::OK, "token order must not matter");
}
