//! PRD-mcphost-funnel-truth
//! AC1 (P0) — Given the migration applied, When a tenant, funnel event, or
//! claim-email row is inserted by any handler, Then it carries `origin` ∈
//! {human, fleet, probe} and never `unknown`.
//!
//! "origin" here is `funnel_origin` (migration 0078) -- a different column
//! than the pre-existing `tenants.origin`/`source_class` (see that
//! migration's own doc comment on why). This test proves the schema shape
//! (additive, `NOT NULL DEFAULT 'unknown'`) and that the three live insert
//! paths this PRD touches -- `control::signup` (tenants), `GET
//! /oauth/authorize` (`oauth_funnel_events`), and a failed claim-email send
//! (`claim_email_events`) -- never leave that default in place.

use crate::common;
use common::TestServer;
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn column_info(conn: &rusqlite::Connection, table: &str, column: &str) -> (bool, Option<String>) {
    conn.query_row(
        &format!("SELECT \"notnull\", dflt_value FROM pragma_table_info('{table}') WHERE name = ?1"),
        rusqlite::params![column],
        |r| Ok((r.get::<_, i64>(0)? != 0, r.get::<_, Option<String>>(1)?)),
    )
    .unwrap_or_else(|e| panic!("{table}.{column} missing from schema: {e}"))
}

#[tokio::test]
async fn funnel_origin_column_is_additive_not_null_default_unknown() {
    let server = TestServer::start().await;
    let db_path = server.data_dir.0.join("mcphost.db");
    let conn = rusqlite::Connection::open(&db_path).expect("open raw db");

    for table in ["tenants", "oauth_funnel_events", "claim_email_events"] {
        let (notnull, default) = column_info(&conn, table, "funnel_origin");
        assert!(notnull, "{table}.funnel_origin must be NOT NULL");
        assert_eq!(
            default.as_deref(),
            Some("'unknown'"),
            "{table}.funnel_origin's migration-time default must be 'unknown'"
        );
    }
}

#[tokio::test]
async fn signup_never_leaves_a_tenant_at_unknown() {
    let server = TestServer::start().await;

    let result = mcphost::control::signup(
        &server.state,
        &json!({"name": "AC1 Tenant"}),
        "8.8.8.8",
        mcphost::control::SignupAttribution::default(),
    )
    .await
    .expect("signup");
    let ns = result["tenant"].as_str().expect("tenant field").to_string();

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("query")
        .expect("tenant exists");
    assert_ne!(tenant.funnel_origin, "unknown", "{tenant:?}");
    assert!(
        ["human", "fleet", "probe"].contains(&tenant.funnel_origin.as_str()),
        "funnel_origin must be one of the three: {tenant:?}"
    );
}

#[tokio::test]
async fn oauth_authorize_hit_never_leaves_a_funnel_event_at_unknown() {
    let server = TestServer::start().await;
    let http = reqwest::Client::new();

    // Deliberately no query params: `record_oauth_funnel_event` runs
    // before any validation, so a bare hit is enough to insert the row
    // this test cares about.
    let _ = http
        .get(format!("{}/oauth/authorize", server.base_url))
        .send()
        .await
        .expect("GET /oauth/authorize");

    let db_path = server.data_dir.0.join("mcphost.db");
    let conn = rusqlite::Connection::open(&db_path).expect("open raw db");
    let funnel_origin: String = conn
        .query_row(
            "SELECT funnel_origin FROM oauth_funnel_events WHERE event = 'authorize_request' \
             ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .expect("query oauth_funnel_events row");
    assert_ne!(funnel_origin, "unknown");
    assert!(["human", "fleet", "probe"].contains(&funnel_origin.as_str()), "{funnel_origin}");
}

#[tokio::test]
async fn claim_email_failure_never_leaves_a_journal_row_at_unknown() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&mock)
        .await;
    let email_config = mcphost::email::EmailConfig {
        api_url: Some(mock.uri()),
        api_key: Some("sentinel-key".to_string()),
        from: Some("claim@mcphost.dev".to_string()),
        provider: mcphost::email::EmailProvider::Resend,
    };
    let email_client: std::sync::Arc<dyn mcphost::email::EmailClient> =
        std::sync::Arc::new(mcphost::email::HttpEmailClient::new(
            reqwest::Client::new(),
            mock.uri(),
            email_config.api_key.clone(),
            mcphost::email::EmailProvider::Resend,
        ));
    let server = TestServer::start_full_with_email(
        Some(common::ADMIN_KEY.to_string()),
        mcphost::kinds::KindRegistry::with_builtin(),
        mcphost::state::CALL_TIMEOUT,
        None,
        mcphost::state::SIGNUP_RATE_LIMIT_PER_HOUR,
        mcphost::billing::BillingConfig::default(),
        std::sync::Arc::new(mcphost::billing::FakeBillingClient::new(mcphost::state::now_unix())),
        Vec::new(),
        email_config,
        email_client,
        mcphost::state::CLAIM_TOKEN_TTL_SECS_DEFAULT,
        mcphost::state::CLAIM_RATE_LIMIT_PER_HOUR_DEFAULT,
        mcphost::db::DbConfig::from_env(),
        mcphost::alerts::AlertConfig::default(),
        mcphost::state::FleetIps::empty(),
        mcphost::state::VerifiedClientIds::empty(),
        Vec::new(),
    )
    .await;

    let anon = common::McpClient::new(&server.base_url);
    let raw = anon
        .tools_call("signup", json!({"name": "AC1 Claim Email Tenant"}))
        .await
        .expect("signup");
    let result = common::extract_structured(&raw);
    let tenant_ns = result["tenant"].as_str().expect("tenant").to_string();
    let claim_url = result["claim_url"].as_str().expect("claim_url").to_string();
    let token = claim_url.rsplit('/').next().expect("claim_url has a path segment");

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(tenant_ns)
        .await
        .expect("query")
        .expect("tenant exists");
    assert_ne!(tenant.funnel_origin, "unknown", "the owning tenant must already be classified");

    let http = reqwest::Client::new();
    let resp = http
        .post(format!("{}/claim/{token}", server.base_url))
        .form(&[("email", "a@b.co")])
        .send()
        .await
        .expect("POST /claim/{token}");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    let db_path = server.data_dir.0.join("mcphost.db");
    let conn = rusqlite::Connection::open(&db_path).expect("open raw db");
    let funnel_origin: String = conn
        .query_row(
            "SELECT funnel_origin FROM claim_email_events WHERE tenant_id = ?1 ORDER BY id DESC LIMIT 1",
            rusqlite::params![tenant.id],
            |r| r.get(0),
        )
        .expect("query claim_email_events row");
    assert_ne!(funnel_origin, "unknown");
    assert_eq!(
        funnel_origin, tenant.funnel_origin,
        "a claim-email journal row must inherit its owning tenant's own funnel_origin"
    );
}
