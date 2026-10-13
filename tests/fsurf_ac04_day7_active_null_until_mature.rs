//! PRD-mcphost-funnel-by-surface-nightly-probe
//! AC4 (P0) — Given a human tenant born 3 days ago, When `day7_active` is
//! computed, Then it is `null` for that tenant's contribution and the
//! surface row's `day7_active` counts only tenants >= 14 days old.

use crate::common::TempDataDir;
use mcphost::auth::{generate_key, generate_namespace, hash_key};
use mcphost::db::{Db, Tenant};
use serde_json::{Value, json};

const DAY: i64 = 86_400;

async fn human(db: &Db, name: &str, client: &str, created_unix: i64) -> Tenant {
    let t = db
        .create_tenant_attributed_with_source(
            name.to_string(),
            generate_namespace(),
            hash_key(&generate_key()),
            None,
            Some("external".to_string()),
            Some(client.to_string()),
            None,
            "external".to_string(),
            None,
            None,
            "human".to_string(),
        )
        .await
        .expect("create tenant");
    db.set_tenant_stamp_for_test(t.id, "created_unix", created_unix).await.expect("backdate");
    t
}

async fn report(db: &Db, now: i64) -> Value {
    let (by_surface, _probe) = mcphost::funnel::surface_report(db, now, 30).await.expect("surface_report");
    by_surface
}

#[tokio::test]
async fn young_tenants_are_null_and_only_mature_tenants_are_counted() {
    let dir = TempDataDir::new();
    let db = Db::open(&dir.0).expect("open db");
    db.migrate().await.expect("migrate");
    let now = mcphost::state::now_unix();

    // Only a 3-day-old human on `vscode`: nothing is measurable yet, so the
    // row reads null (not 0) even though the tenant has signed up.
    let young = human(&db, "young", "Visual Studio Code", now - 3 * DAY).await;
    db.insert_call_row_for_test_full(young.id, "t".into(), None, None, true, 1, now - DAY)
        .await
        .expect("young call");
    let by_surface = report(&db, now).await;
    assert_eq!(by_surface["vscode"]["signed_up"], json!(1), "{by_surface}");
    assert_eq!(by_surface["vscode"]["day7_active"], Value::Null, "{by_surface}");

    // A 20-day-old tenant active on day 9, and a 20-day-old one that never
    // came back, join the same row: only those two are measured, one active.
    let old_active = human(&db, "old-active", "Visual Studio Code", now - 20 * DAY).await;
    db.insert_call_row_for_test_full(old_active.id, "t".into(), None, None, true, 1, now - 20 * DAY + 9 * DAY)
        .await
        .expect("day 9 call");
    let _old_idle = human(&db, "old-idle", "Visual Studio Code", now - 20 * DAY).await;
    let by_surface = report(&db, now).await;
    assert_eq!(by_surface["vscode"]["signed_up"], json!(3), "{by_surface}");
    assert_eq!(by_surface["vscode"]["day7_active"], json!(1), "{by_surface}");
}
