//! PRD-mcphost-external-class-positive-evidence
//! AC3 (P0) — Given an `unattributed` tenant created at `created_unix` on UTC
//! date D with no fleet-IP or header-stamped call in its history, When it
//! makes an authenticated non-loopback, non-fleet, header-less call on UTC
//! date D+1 or later and at least 21,600 s after `created_unix`, Then
//! `origin='external'` with detail `returned:<n>d` and the audit detail
//! carries the source IP hash; a call 15 s after creation across UTC
//! midnight, a fleet-IP call, a header-stamped call, or any tenant whose
//! history holds one such call never promotes.
//!
//! Peer addresses are injected through `X-Forwarded-For` (honoured because
//! every connection to the test server is a loopback peer).

use crate::common;
use common::{McpClient, TestServer};
use mcphost::control::{SignupAttribution, signup};
use mcphost::state::{now_unix, returned_days};
use serde_json::{Value, json};

const FLEET_IP: &str = "198.51.100.7";
const PUBLIC_IP: &str = "203.0.113.31";

struct Subject {
    ns: String,
    client: McpClient,
}

async fn server() -> TestServer {
    TestServer::start_with_signup_rate_limit_and_fleet_ips(1000, FLEET_IP).await
}

/// An `unattributed` tenant whose signup was `age_secs` ago.
async fn subject(server: &TestServer, name: &str, age_secs: i64) -> Subject {
    let result = signup(&server.state, &json!({"name": name}), "203.0.113.30", SignupAttribution::default())
        .await
        .expect("signup");
    let ns = result["tenant"].as_str().unwrap().to_string();
    let key = result["key"].as_str().unwrap().to_string();
    let t = server.state.db.find_tenant_by_namespace(ns.clone()).await.unwrap().unwrap();
    assert_eq!(t.origin, "unattributed");
    server
        .state
        .db
        .set_claim_timing_for_test(t.id, "unattributed".to_string(), now_unix() - age_secs, None, None)
        .await
        .unwrap();
    Subject { ns, client: McpClient::with_bearer(&server.base_url, &key) }
}

impl Subject {
    async fn call(&self, headers: &[(&str, &str)]) {
        self.client
            .tools_call_with_headers("host.whoami", json!({}), headers)
            .await
            .unwrap_or_else(|e| panic!("whoami: {} {}", e.code, e.message));
    }

    async fn origin(&self, server: &TestServer) -> (String, Option<String>) {
        let t = server.state.db.find_tenant_by_namespace(self.ns.clone()).await.unwrap().unwrap();
        (t.origin, t.origin_detail)
    }
}

async fn promotions(server: &TestServer) -> Vec<mcphost::db::AdminAuditRow> {
    server
        .state
        .db
        .list_admin_audit(500, None)
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.action == "origin_promoted")
        .collect()
}

const TWO_DAYS: i64 = 2 * 86_400;

#[tokio::test]
async fn mcphost_external_class_positive_evidence_ac03_clean_later_day_call_promotes_with_ip_hash() {
    let server = server().await;
    let s = subject(&server, "AC3 Returner", TWO_DAYS).await;
    s.call(&[("x-forwarded-for", PUBLIC_IP)]).await;

    let (origin, detail) = s.origin(&server).await;
    assert_eq!(origin, "external");
    let detail = detail.expect("detail");
    assert!(detail.starts_with("returned:") && detail.ends_with('d'), "{detail}");
    assert!(["returned:2d", "returned:3d"].contains(&detail.as_str()), "{detail}");

    let rows = promotions(&server).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    let audit: Value = serde_json::from_str(rows[0].detail.as_deref().unwrap()).unwrap();
    assert_eq!(audit["from"], json!("unattributed"));
    assert_eq!(audit["to"], json!("external"));
    assert_eq!(audit["detail"], json!(detail));
    let hash = audit["ip_hash"].as_str().expect("ip_hash in audit detail");
    assert!(!hash.is_empty() && !hash.contains(PUBLIC_IP), "{audit}");
    assert!(!rows[0].detail.as_deref().unwrap().contains(PUBLIC_IP));

    // Further clean calls change nothing and write no second row.
    s.call(&[("x-forwarded-for", PUBLIC_IP)]).await;
    assert_eq!(promotions(&server).await.len(), 1);
}

#[tokio::test]
async fn mcphost_external_class_positive_evidence_ac03_same_day_or_early_call_never_promotes() {
    let server = server().await;
    // 1 h old: under the 6 h floor whatever the UTC date is.
    let s = subject(&server, "AC3 Early", 3_600).await;
    s.call(&[("x-forwarded-for", PUBLIC_IP)]).await;
    assert_eq!(s.origin(&server).await.0, "unattributed");
    assert!(promotions(&server).await.is_empty());

    // The rule itself, pinned on fixed instants: 15 s across midnight is no
    // return; 6 h on the next date is; same date never is.
    let midnight = 20_000 * 86_400;
    assert_eq!(returned_days(midnight - 10, midnight + 5), None);
    assert_eq!(returned_days(midnight - 10, midnight - 10 + 21_599), None);
    assert_eq!(returned_days(midnight - 3_600, midnight + 21_600), Some(1));
    assert_eq!(returned_days(midnight - 10, midnight + 21_600), Some(1));
    assert_eq!(returned_days(midnight, midnight + 86_399), None);
    assert_eq!(returned_days(midnight, midnight + 3 * 86_400 + 7), Some(3));
}

#[tokio::test]
async fn mcphost_external_class_positive_evidence_ac03_fleet_ip_call_blocks_for_good() {
    let server = server().await;
    let s = subject(&server, "AC3 Fleet", TWO_DAYS).await;
    s.call(&[("x-forwarded-for", FLEET_IP)]).await;
    assert_eq!(s.origin(&server).await.0, "unattributed");
    // The tenant's history now holds a fleet-IP call: a clean later-day call
    // no longer promotes it.
    s.call(&[("x-forwarded-for", PUBLIC_IP)]).await;
    assert_eq!(s.origin(&server).await.0, "unattributed");
    assert!(promotions(&server).await.is_empty());
}

#[tokio::test]
async fn mcphost_external_class_positive_evidence_ac03_header_stamped_call_blocks_for_good() {
    let server = server().await;
    let s = subject(&server, "AC3 Stamped", TWO_DAYS).await;
    s.call(&[("x-forwarded-for", PUBLIC_IP), ("x-mcphost-synthetic", "seat-script")]).await;
    assert_eq!(s.origin(&server).await.0, "unattributed");
    s.call(&[("x-forwarded-for", PUBLIC_IP)]).await;
    assert_eq!(s.origin(&server).await.0, "unattributed");
    assert!(promotions(&server).await.is_empty());
}

#[tokio::test]
async fn mcphost_external_class_positive_evidence_ac03_loopback_call_never_promotes() {
    let server = server().await;
    let s = subject(&server, "AC3 Loopback", TWO_DAYS).await;
    s.call(&[]).await; // no forwarded address: the loopback peer itself
    s.call(&[("x-forwarded-for", PUBLIC_IP)]).await;
    assert_eq!(s.origin(&server).await.0, "unattributed");
    assert!(promotions(&server).await.is_empty());
}
