//! PRD-mcphost-slo-targets-on-status AC5 (P0) — Given `calls` rows whose
//! `error_class` is the code of a `class == HostFault` registry entry and
//! rows whose `error_class` is a tenant-fault code, When the error rate is
//! computed, Then host-fault rows count in numerator and denominator,
//! tenant-fault rows in the denominator only, and a test asserts the
//! host-fault set derived from `ERROR_CODES` is non-empty.

use crate::common::{TestServer, signup};
use mcphost::error_registry::{ERROR_CODES, ErrorClass, lookup};
use mcphost::statusfeed::host_fault_codes;
use serde_json::Value;

#[test]
fn slopg_ac05_host_fault_set_is_derived_from_the_registry_and_non_empty() {
    let set = host_fault_codes();
    assert!(!set.is_empty(), "a hand-written class list would read 0 forever; the derived set must not be empty");
    let expected: Vec<&str> = ERROR_CODES.iter().filter(|e| e.class == ErrorClass::HostFault).map(|e| e.code).collect();
    assert_eq!(set, expected, "the set is exactly the registry's HostFault entries");
    for code in &set {
        let entry = lookup(code).unwrap_or_else(|| panic!("`{code}` is not a registry code"));
        assert_eq!(entry.class, ErrorClass::HostFault, "{code}");
    }
    // The wire codes a tenant's own mistake produces are not in the set.
    for tenant_code in ["tool_exception", "args_invalid", "upstream_status"] {
        assert!(!set.contains(&tenant_code), "{tenant_code} must never count against the host");
    }
}

#[tokio::test]
async fn slopg_ac05_host_faults_in_numerator_and_denominator_tenant_faults_in_denominator_only() {
    let server = TestServer::start().await;
    let (ns, _key) = signup(&server.base_url, "Slo Rate").await;
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("find").expect("tenant");
    let now = mcphost::state::now_unix();
    let db = &server.state.db;

    let faults = host_fault_codes();
    let mut host_rows = 0_i64;
    let mut at = now - 60;
    // Two rows per host-fault code.
    for code in &faults {
        for _ in 0..2 {
            at -= 1;
            db.insert_slo_call_row_for_test(tenant.id, at, 40, false, Some((*code).to_string()), "human").await.expect("host fault");
            host_rows += 1;
        }
    }
    // Tenant-fault rows: three codes, and a class name that is no wire code at all.
    let mut tenant_rows = 0_i64;
    for code in ["tool_exception", "args_invalid", "upstream_status", "refused"] {
        for _ in 0..5 {
            at -= 1;
            db.insert_slo_call_row_for_test(tenant.id, at, 40, false, Some(code.to_string()), "human").await.expect("tenant fault");
            tenant_rows += 1;
        }
    }
    // Clean rows pad the denominator.
    let ok_rows = 40_i64;
    for _ in 0..ok_rows {
        at -= 1;
        db.insert_slo_call_row_for_test(tenant.id, at, 40, true, None, "human").await.expect("ok");
    }
    // Probe-origin host faults and rows older than 24 h are outside the measurement.
    db.insert_slo_call_row_for_test(tenant.id, now - 30, 40, false, Some(faults[0].to_string()), "probe").await.expect("probe row");
    db.insert_slo_call_row_for_test(tenant.id, now - 2 * 86_400, 40, false, Some(faults[0].to_string()), "human").await.expect("old row");

    let total = host_rows + tenant_rows + ok_rows;
    let body: Value = reqwest::Client::new()
        .get(format!("{}/status.json", server.base_url))
        .send()
        .await
        .expect("GET /status.json")
        .json()
        .await
        .expect("parse body");
    let rate = &body["slo"]["error_rate_24h"];
    let expected = host_rows as f64 / total as f64 * 100.0;
    assert_eq!(rate["sample_n"].as_i64(), Some(total), "every non-probe row is in the denominator: {rate}");
    let measured = rate["measured"].as_f64().expect("measured");
    assert!((measured - expected).abs() < 1e-9, "measured {measured} != {expected}: {rate}");
    assert!(measured > 0.0 && measured < 100.0 * (host_rows + tenant_rows) as f64 / total as f64, "tenant faults add nothing: {rate}");
}
