//! PRD-mcphost-external-class-positive-evidence
//! AC3 — Given an `unattributed` tenant created at `created_unix` on UTC date
//! D with no fleet-IP or header-stamped call in its history, When it makes an
//! authenticated non-loopback, non-fleet, header-less call on UTC date D+1 or
//! later and at least 21,600 s after `created_unix`, Then `origin='external'`
//! with detail `returned:<days>d` and the audit detail carries the source IP
//! hash; a call 15 s after creation across UTC midnight, a fleet-IP call, a
//! header-stamped call, or any tenant whose history holds one such call never
//! promotes.

use crate::common;
use common::{McpClient, TestServer};
use mcphost::db::{OriginCallObservation, PromoteOutcome};
use serde_json::json;

const DAY: i64 = 86_400;
const FLEET_IP: &str = "198.51.100.77";

/// A UTC-midnight anchor well inside the test clock's range.
fn midnight() -> i64 {
    let t = mcphost::state::now_unix();
    t - t.rem_euclid(DAY)
}

async fn unattributed_tenant(server: &TestServer, name: &str, created_unix: i64) -> (i64, String, String) {
    let result = mcphost::control::signup(
        &server.state,
        &json!({"name": name}),
        "203.0.113.20",
        mcphost::control::SignupAttribution::default(),
    )
    .await
    .expect("signup");
    let ns = result["tenant"].as_str().unwrap().to_string();
    let key = result["key"].as_str().expect("key").to_string();
    let t = server.state.db.find_tenant_by_namespace(ns.clone()).await.unwrap().unwrap();
    server
        .state
        .db
        .set_claim_timing_for_test(t.id, "unattributed".to_string(), created_unix, None, None)
        .await
        .unwrap();
    (t.id, ns, key)
}

async fn origin_of(server: &TestServer, ns: &str) -> (String, Option<String>) {
    let t = server.state.db.find_tenant_by_namespace(ns.to_string()).await.unwrap().unwrap();
    (t.origin, t.origin_detail)
}

fn clean() -> OriginCallObservation {
    OriginCallObservation::Clean { ip_hash: "hash-abc".to_string() }
}

#[tokio::test]
async fn mcphost_external_class_positive_evidence_ac03() {
    let server = TestServer::start().await;
    let d = midnight() - 10 * DAY;

    // 15 s after creation, across UTC midnight: never a return.
    let (id, ns, _) = unattributed_tenant(&server, "AC3 Midnight", d + DAY - 15).await;
    let out = server.state.db.observe_origin_call(id, d + DAY, clean()).await.unwrap();
    assert_eq!(out, PromoteOutcome::Unchanged);
    assert_eq!(origin_of(&server, &ns).await.0, "unattributed");

    // Next UTC date but under the 6 h floor: not yet.
    let (id, ns, _) = unattributed_tenant(&server, "AC3 Floor", d + 20 * 3600).await;
    let out = server.state.db.observe_origin_call(id, d + DAY + 3600, clean()).await.unwrap();
    assert_eq!(out, PromoteOutcome::Unchanged);
    assert_eq!(origin_of(&server, &ns).await.0, "unattributed");
    // ... and the same tenant exactly 6 h after creation, on D+1: promoted.
    let out = server.state.db.observe_origin_call(id, d + DAY + 2 * 3600, clean()).await.unwrap();
    assert_eq!(out, PromoteOutcome::Promoted);
    let (origin, detail) = origin_of(&server, &ns).await;
    assert_eq!((origin.as_str(), detail.as_deref()), ("external", Some("returned:1d")));

    // Clean later-day call: promoted once, audit carries the IP hash.
    let (id, ns, _) = unattributed_tenant(&server, "AC3 Return", d + 3600).await;
    let out = server.state.db.observe_origin_call(id, d + 3 * DAY + 3600, clean()).await.unwrap();
    assert_eq!(out, PromoteOutcome::Promoted);
    let (origin, detail) = origin_of(&server, &ns).await;
    assert_eq!((origin.as_str(), detail.as_deref()), ("external", Some("returned:3d")));
    let rows: Vec<_> = server
        .state
        .db
        .list_admin_audit(1000, None)
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.action == "origin_promoted" && r.target.as_deref() == Some(ns.as_str()))
        .collect();
    assert_eq!(rows.len(), 1);
    let detail = rows[0].detail.as_deref().unwrap();
    assert!(detail.contains("detail=returned:3d") && detail.contains("ip_hash=hash-abc"), "{detail}");
    // A second clean call changes nothing and writes no second row.
    let out = server.state.db.observe_origin_call(id, d + 4 * DAY, clean()).await.unwrap();
    assert_eq!(out, PromoteOutcome::Unchanged);

    // Any fleet-IP or header-stamped call in the history blocks it for good.
    for kind in ["fleet-ip", "header"] {
        let (id, ns, _) = unattributed_tenant(&server, &format!("AC3 Taint {kind}"), d + 3600).await;
        server.state.db.observe_origin_call(id, d + 100, OriginCallObservation::Tainted(kind)).await.unwrap();
        let out = server.state.db.observe_origin_call(id, d + 5 * DAY, clean()).await.unwrap();
        assert_eq!(out, PromoteOutcome::Unchanged, "{kind}");
        assert_eq!(origin_of(&server, &ns).await.0, "unattributed", "{kind}");
    }
}

#[tokio::test]
async fn mcphost_external_class_positive_evidence_ac03_through_the_call_path() {
    let server = TestServer::start_with_signup_rate_limit_and_fleet_ips(1000, FLEET_IP).await;
    let old = mcphost::state::now_unix() - 2 * DAY;

    // A clean forwarded call two days after signup promotes.
    let (_, ns, key) = unattributed_tenant(&server, "AC3 Path Return", old).await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call_with_headers("host.whoami", json!({}), &[("x-forwarded-for", "203.0.113.31")])
        .await
        .expect("whoami");
    let (origin, detail) = origin_of(&server, &ns).await;
    assert_eq!(origin, "external");
    assert!(detail.as_deref().is_some_and(|d| d.starts_with("returned:") && d.ends_with('d')), "{detail:?}");

    // A fleet-IP call first taints; the later clean call never promotes.
    let (_, ns, key) = unattributed_tenant(&server, "AC3 Path Fleet", old).await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call_with_headers("host.whoami", json!({}), &[("x-forwarded-for", FLEET_IP)])
        .await
        .expect("whoami fleet");
    client
        .tools_call_with_headers("host.whoami", json!({}), &[("x-forwarded-for", "203.0.113.32")])
        .await
        .expect("whoami clean");
    assert_eq!(origin_of(&server, &ns).await.0, "unattributed");

    // A header-stamped call taints the same way.
    let (_, ns, key) = unattributed_tenant(&server, "AC3 Path Header", old).await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call_with_headers(
            "host.whoami",
            json!({}),
            &[("x-forwarded-for", "203.0.113.33"), ("x-mcphost-synthetic", "seat:probe")],
        )
        .await
        .expect("whoami header");
    client
        .tools_call_with_headers("host.whoami", json!({}), &[("x-forwarded-for", "203.0.113.34")])
        .await
        .expect("whoami clean");
    assert_eq!(origin_of(&server, &ns).await.0, "unattributed");
}
