//! PRD-mcphost-external-class-positive-evidence
//! AC3 — Given an `unattributed` tenant created at `created_unix` on UTC
//! date D with no fleet-IP or header-stamped call in its history, When it
//! makes an authenticated non-loopback, non-fleet, header-less call on UTC
//! date D+1 or later and at least 21,600 s after `created_unix`, Then
//! `origin='external'` with detail `returned:<days>` and the audit detail
//! carries the source IP hash; a call 15 s after creation across UTC
//! midnight, a fleet-IP call, a header-stamped call, or any tenant whose
//! history holds one such call never promotes.

use crate::common;
use common::{McpClient, TestServer};
use serde_json::{Value, json};

const DAY: i64 = 86_400;

struct Fresh {
    id: i64,
    namespace: String,
    key: String,
}

async fn fresh(server: &TestServer, name: &str, ip: &str, created_unix: i64) -> Fresh {
    let result = mcphost::control::signup(
        &server.state,
        &json!({ "name": name }),
        ip,
        mcphost::control::SignupAttribution::default(),
    )
    .await
    .expect("non-fleet signup");
    let namespace = result["tenant"].as_str().expect("tenant").to_string();
    let key = result["key"].as_str().expect("key").to_string();
    let tenant = server.state.db.find_tenant_by_namespace(namespace.clone()).await.unwrap().unwrap();
    assert_eq!(tenant.origin, "unattributed");
    server
        .state
        .db
        .set_claim_timing_for_test(tenant.id, "unattributed".to_string(), created_unix, None, None)
        .await
        .unwrap();
    Fresh { id: tenant.id, namespace, key }
}

async fn row(server: &TestServer, f: &Fresh) -> (String, Option<String>) {
    let t = server.state.db.find_tenant_by_namespace(f.namespace.clone()).await.unwrap().unwrap();
    (t.origin, t.origin_detail)
}

async fn promoted_rows(server: &TestServer, namespace: &str) -> Vec<mcphost::db::AdminAuditRow> {
    server
        .state
        .db
        .list_admin_audit(1000, None)
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.action == "origin_promoted" && r.target.as_deref() == Some(namespace))
        .collect()
}

async fn observe(server: &TestServer, f: &Fresh, created: i64, now: i64, fleet_or_stamped: bool) {
    server
        .state
        .db
        .observe_call_origin(f.id, "unattributed", created, now, fleet_or_stamped, "hash-abc".to_string())
        .await
        .unwrap();
}

#[tokio::test]
async fn mcphost_external_class_positive_evidence_ac03_http_return_promotes_with_ip_hash() {
    let server = TestServer::start().await;
    let now = mcphost::state::now_unix();
    let a = fresh(&server, "AC3 Returner", "203.0.113.31", now - 3 * DAY).await;

    let client = McpClient::with_bearer(&server.base_url, &a.key);
    client
        .tools_call_with_headers("host.whoami", json!({}), &[("x-forwarded-for", "198.51.100.9")])
        .await
        .expect("authenticated call");

    let (origin, detail) = row(&server, &a).await;
    assert_eq!(origin, "external");
    let detail = detail.unwrap();
    assert!(detail.starts_with("returned:") && detail.ends_with('d'), "{detail}");
    let rows = promoted_rows(&server, &a.namespace).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    let audit = rows[0].detail.as_deref().unwrap();
    assert!(audit.contains("from=unattributed") && audit.contains("to=external"), "{audit}");
    assert!(audit.contains("ip_hash="), "{audit}");
    assert!(!audit.contains("198.51.100.9"), "raw IP leaked into the audit row: {audit}");

    let health: Value = reqwest::Client::new()
        .get(format!("{}/healthz", server.base_url))
        .bearer_auth(common::ADMIN_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(health["external_by_evidence"]["returned"], json!(1), "{health:?}");
    // `returned` never enters `tenants_confirmed`.
    assert_eq!(health["tenants_confirmed"], json!(0), "{health:?}");
}

#[tokio::test]
async fn mcphost_external_class_positive_evidence_ac03_http_stamped_history_never_promotes() {
    let server = TestServer::start().await;
    let now = mcphost::state::now_unix();
    let b = fresh(&server, "AC3 Stamped", "203.0.113.32", now - 3 * DAY).await;

    let client = McpClient::with_bearer(&server.base_url, &b.key);
    client
        .tools_call_with_headers(
            "host.whoami",
            json!({}),
            &[("x-forwarded-for", "198.51.100.10"), ("x-mcphost-synthetic", "harness:seat-script")],
        )
        .await
        .expect("stamped call");
    client
        .tools_call_with_headers("host.whoami", json!({}), &[("x-forwarded-for", "198.51.100.10")])
        .await
        .expect("clean later call");

    let (origin, _) = row(&server, &b).await;
    assert_eq!(origin, "unattributed", "one stamped call in the history blocks promotion for good");
    assert!(promoted_rows(&server, &b.namespace).await.is_empty());
}

#[tokio::test]
async fn mcphost_external_class_positive_evidence_ac03_utc_date_and_six_hour_floor() {
    let server = TestServer::start().await;
    let midnight = 20_000 * DAY;

    // Created 15 s before UTC midnight, called 15 s later: a later date, but
    // not a return.
    let c = fresh(&server, "AC3 Midnight", "203.0.113.33", midnight - 15).await;
    observe(&server, &c, midnight - 15, midnight, false).await;
    assert_eq!(row(&server, &c).await.0, "unattributed");

    // Same UTC date, 23 h after creation: not a later date.
    let d = fresh(&server, "AC3 Same Day", "203.0.113.34", midnight).await;
    observe(&server, &d, midnight, midnight + DAY - 1, false).await;
    assert_eq!(row(&server, &d).await.0, "unattributed");

    // Later date but 5 h after creation: under the 6 h floor.
    let e = fresh(&server, "AC3 Five Hours", "203.0.113.35", midnight - 4 * 3600).await;
    observe(&server, &e, midnight - 4 * 3600, midnight + 3600, false).await;
    assert_eq!(row(&server, &e).await.0, "unattributed");

    // Later date and >= 6 h: promoted, `returned:1d`.
    observe(&server, &e, midnight - 4 * 3600, midnight + 2 * 3600, false).await;
    let (origin, detail) = row(&server, &e).await;
    assert_eq!((origin.as_str(), detail.as_deref()), ("external", Some("returned:1d")));
    assert_eq!(promoted_rows(&server, &e.namespace).await.len(), 1);
}

#[tokio::test]
async fn mcphost_external_class_positive_evidence_ac03_fleet_call_in_history_never_promotes() {
    let server = TestServer::start().await;
    let midnight = 20_000 * DAY;
    let f = fresh(&server, "AC3 Fleet Touch", "203.0.113.36", midnight - 10 * 3600).await;

    // A fleet-IP (or stamped) call on day D...
    observe(&server, &f, midnight - 10 * 3600, midnight - 9 * 3600, true).await;
    // ...then clean calls on D+1 and D+2 never promote.
    observe(&server, &f, midnight - 10 * 3600, midnight + 8 * 3600, false).await;
    observe(&server, &f, midnight - 10 * 3600, midnight + DAY + 8 * 3600, false).await;
    assert_eq!(row(&server, &f).await.0, "unattributed");
    assert!(promoted_rows(&server, &f.namespace).await.is_empty());
}
