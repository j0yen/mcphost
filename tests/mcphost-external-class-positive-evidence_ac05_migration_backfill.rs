//! PRD-mcphost-external-class-positive-evidence
//! AC5 — Given one seeded tenant per case (external+verified, external+
//! later-day call only, external+same-day only, unclassified,
//! synthetic+verified), When the migration runs, Then the verified row is
//! `external`/`claimed-email`, both unverified external rows are
//! `unattributed`/`legacy-external-unverified`, the unclassified row is
//! `unattributed`/`legacy-unclassified`, the synthetic row is unchanged, no
//! row is NULL, and the journaled before and after counts sum to the same
//! total. (`internal` is round-tripped untouched.)

use crate::common;
use common::TempDataDir;
use mcphost::auth::{generate_key, generate_namespace, hash_key};
use mcphost::db::Db;
use serde_json::Value;

const DAY: i64 = 86_400;

async fn seed(db: &Db, name: &str, origin: &str, detail: &str, created: i64, verified: Option<i64>) -> String {
    let namespace = generate_namespace();
    let tenant = db
        .create_tenant_attributed(
            name.to_string(),
            namespace.clone(),
            hash_key(&generate_key()),
            None,
            None,
            None,
            None,
            origin.to_string(),
            Some(detail.to_string()),
        )
        .await
        .expect("seed tenant");
    db.set_claim_timing_for_test(tenant.id, origin.to_string(), created, verified, None)
        .await
        .expect("timing");
    namespace
}

async fn row(db: &Db, namespace: &str) -> (String, Option<String>) {
    let t = db.find_tenant_by_namespace(namespace.to_string()).await.unwrap().unwrap();
    (t.origin, t.origin_detail)
}

#[tokio::test]
async fn mcphost_external_class_positive_evidence_ac05_backfill_partitions_legacy_rows() {
    let dir = TempDataDir::new();
    let db = Db::open(&dir.0).expect("open db");
    db.migrate().await.expect("migrate");

    let now = mcphost::state::now_unix();
    let verified = seed(&db, "verified", "external", "external-unverified", now - 5 * DAY, Some(now - DAY)).await;
    let later_day = seed(&db, "later day", "external", "external-unverified", now - 5 * DAY, None).await;
    let same_day = seed(&db, "same day", "external", "external-unverified", now, None).await;
    let unclassified = seed(&db, "unclassified", "unclassified", "x", now - 5 * DAY, None).await;
    let synthetic = seed(&db, "synthetic", "synthetic", "harness:fleet-ip", now - 5 * DAY, Some(now - DAY)).await;
    let internal = seed(&db, "perf-free", "internal", "perf-free", now - 5 * DAY, None).await;
    // The "later-day call" row really did call on a later UTC day.
    let id = db.find_tenant_by_namespace(later_day.clone()).await.unwrap().unwrap().id;
    db.touch_last_seen(id, now - 4 * DAY).await.unwrap();

    db.clear_origin_backfill_marker_for_test().await.unwrap();
    db.migrate().await.expect("migrate with legacy rows");

    assert_eq!(row(&db, &verified).await, ("external".into(), Some("claimed-email".into())));
    for ns in [&later_day, &same_day] {
        assert_eq!(row(&db, ns).await, ("unattributed".into(), Some("legacy-external-unverified".into())));
    }
    assert_eq!(row(&db, &unclassified).await, ("unattributed".into(), Some("legacy-unclassified".into())));
    assert_eq!(row(&db, &synthetic).await, ("synthetic".into(), Some("harness:fleet-ip".into())));
    assert_eq!(row(&db, &internal).await, ("internal".into(), Some("perf-free".into())));

    // No NULL origin, and every origin is a member of the enum.
    let report = db.origin_report().await.unwrap();
    assert_eq!(report.unknown, 0, "{report:?}");

    // The journal: before and after sum to the same total.
    let audit = db.list_admin_audit(1000, None).await.unwrap();
    let journal = audit.iter().find(|r| r.action == "origin_backfill").expect("origin_backfill journal row");
    let j: Value = serde_json::from_str(journal.detail.as_deref().unwrap()).unwrap();
    let sum = |v: &Value| v.as_object().unwrap().values().map(|n| n.as_i64().unwrap()).sum::<i64>();
    assert_eq!(sum(&j["before"]), 6);
    assert_eq!(sum(&j["before"]), sum(&j["after"]), "{j}");
    assert_eq!(j["before"]["external"], 3);
    assert_eq!(j["before"]["unclassified"], 1);
    assert_eq!(j["after"]["external"], 1);
    assert_eq!(j["after"]["unattributed"], 3);

    // Once only: a second migrate must not demote the earned `external` row
    // or re-journal.
    db.migrate().await.expect("second migrate");
    assert_eq!(row(&db, &verified).await.0, "external");
    let again = db.list_admin_audit(1000, None).await.unwrap();
    assert_eq!(again.iter().filter(|r| r.action == "origin_backfill").count(), 1);
}
