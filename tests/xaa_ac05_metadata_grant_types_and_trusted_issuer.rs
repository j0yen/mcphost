//! PRD-mcphost-enterprise-managed-auth
//! AC5 (P0) — Given the per-tenant and root authorization-server metadata,
//! When read, Then `grant_types_supported` includes the JWT-bearer URN and
//! the per-tenant document lists acme's trusted issuer; the harness `wif`
//! scenarios (`expired-assertion`, `wrong-audience`, `scope-rejected`,
//! `grant-fallback`) read `pass`.
//!
//! The second clause's "harness" is `mcphost::oauthclient::run_wif_scenarios`
//! (also reachable as the `mcphost oauth-probe` CLI subcommand) --
//! `wif_scenarios_all_read_pass` below drives it against this test's own
//! real, DB-backed server exactly as an operator would against prod (AC10),
//! and fails if any scenario reports anything but `"pass"`.

use crate::common;
use common::{McpClient, TestServer, signup};

use crate::oauth;
use oauth::{KID_1, jwk_1, jwks_server, priv_pem_1};

use mcphost::oauthclient::{WifProbeConfig, run_wif_scenarios};

use serde_json::{Value, json};

#[tokio::test]
async fn root_and_per_tenant_metadata_advertise_jwt_bearer_and_trusted_issuer() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Acme Corp").await;
    let key_client = McpClient::with_bearer(&server.base_url, &key);

    let jwks = jwks_server(jwk_1()).await;
    let issuer = "https://idp.acme-test.example.com";
    key_client
        .tools_call(
            "host.oauth.trusted_issuer_set",
            json!({
                "issuer": issuer,
                "jwks_url": format!("{}/jwks", jwks.uri()),
                "client_id": "claude-enterprise",
            }),
        )
        .await
        .expect("trusted_issuer_set must succeed");

    let http = reqwest::Client::new();

    let root: Value = http
        .get(format!("{}/.well-known/oauth-authorization-server", server.base_url))
        .send()
        .await
        .expect("GET root metadata")
        .json()
        .await
        .expect("parse root metadata");
    let root_grant_types = root["grant_types_supported"].as_array().expect("grant_types_supported array");
    assert!(
        root_grant_types.iter().any(|v| v == "urn:ietf:params:oauth:grant-type:jwt-bearer"),
        "root metadata must advertise the JWT-bearer grant: {root_grant_types:?}"
    );

    let root_openid: Value = http
        .get(format!("{}/.well-known/openid-configuration", server.base_url))
        .send()
        .await
        .expect("GET root openid-configuration")
        .json()
        .await
        .expect("parse root openid-configuration");
    assert_eq!(root_openid, root, "openid-configuration must mirror oauth-authorization-server");

    for path in ["oauth-authorization-server", "openid-configuration"] {
        let doc: Value = http
            .get(format!("{}/.well-known/{path}/t/{ns}/mcp", server.base_url))
            .send()
            .await
            .expect("GET per-tenant metadata")
            .json()
            .await
            .expect("parse per-tenant metadata");
        let tenant_grant_types = doc["grant_types_supported"].as_array().expect("grant_types_supported array");
        assert!(
            tenant_grant_types.iter().any(|v| v == "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            "per-tenant metadata ({path}) must advertise the JWT-bearer grant: {tenant_grant_types:?}"
        );
        let issuers = doc["identity_assertion_issuers_supported"].as_array().expect("issuers array");
        assert!(
            issuers.iter().any(|v| v == issuer),
            "per-tenant metadata ({path}) must list acme's trusted issuer: {issuers:?}"
        );
    }

    // An unknown namespace is 404, not a leak of which namespaces exist.
    let status = http
        .get(format!("{}/.well-known/oauth-authorization-server/t/nope/mcp", server.base_url))
        .send()
        .await
        .expect("GET unknown-namespace metadata")
        .status();
    assert_eq!(status, reqwest::StatusCode::NOT_FOUND, "unknown namespace must 404");
}

#[tokio::test]
async fn wif_scenarios_all_read_pass() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Acme Corp").await;
    let key_client = McpClient::with_bearer(&server.base_url, &key);

    let jwks = jwks_server(jwk_1()).await;
    let issuer = "https://idp.acme-test.example.com";
    key_client
        .tools_call(
            "host.oauth.trusted_issuer_set",
            json!({
                "issuer": issuer,
                "jwks_url": format!("{}/jwks", jwks.uri()),
                "client_id": "claude-enterprise",
            }),
        )
        .await
        .expect("trusted_issuer_set must succeed");

    let resource = format!("{}/t/{ns}/mcp", server.base_url);
    let cfg = WifProbeConfig {
        resource: &resource,
        issuer,
        client_id: "claude-enterprise",
        kid: KID_1,
        priv_pem: priv_pem_1(),
        tenant_key: &key,
    };
    let http = reqwest::Client::new();
    let reports = run_wif_scenarios(&http, &server.base_url, &cfg).await;

    for report in &reports {
        assert!(report.passed(), "wif scenario {} must read pass: {}", report.scenario, report.detail);
    }
    let scenarios: Vec<&str> = reports.iter().map(|r| r.scenario).collect();
    assert_eq!(
        scenarios,
        vec!["expired-assertion", "wrong-audience", "scope-rejected", "grant-fallback"],
        "the full wif family must run, in the harness's own order"
    );
}
