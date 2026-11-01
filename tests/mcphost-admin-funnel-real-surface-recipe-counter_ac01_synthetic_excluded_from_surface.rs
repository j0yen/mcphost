//! PRD-mcphost-admin-funnel-real-surface-recipe-counter
//! AC1 (P0) — Given human-origin tenants that are synthetic (a `synthorg`
//! client name, `source_class` or `origin` synthetic) beside one ordinary
//! tenant, and an empty window, When `surface_report` runs, Then the
//! synthetic ones are in no surface row, `synthetic_excluded` equals their
//! count, the ordinary one counts once, and the empty window gives zero
//! cells with `day7_active` null.

use crate::common::TempDataDir;
use mcphost::auth::{generate_key, generate_namespace, hash_key};
use mcphost::db::Db;
use serde_json::Value;

async fn tenant(db: &Db, name: &str, client: &str, source_class: &str, origin: &str) -> i64 {
    db.create_tenant_attributed_with_source(
        name.to_string(),
        generate_namespace(),
        hash_key(&generate_key()),
        None,
        Some(source_class.to_string()),
        Some(client.to_string()),
        None,
        origin.to_string(),
        None,
        None,
        "human".to_string(),
    )
    .await
    .expect("create tenant")
    .id
}

fn total_signed_up(by_surface: &Value) -> i64 {
    by_surface.as_object().expect("object").values().map(|r| r["signed_up"].as_i64().unwrap()).sum()
}

#[tokio::test]
async fn synthetic_human_tenants_leave_every_surface_row() {
    let dir = TempDataDir::new();
    let db = Db::open(&dir.0).expect("open db");
    db.migrate().await.expect("migrate");
    let now = mcphost::state::now_unix();

    // Empty window first: zero cells, day7_active null, nothing excluded.
    let (empty, _, excluded) = mcphost::funnel::surface_report(&db, now, 7).await.expect("report");
    assert_eq!(excluded, 0);
    for (surface, row) in empty.as_object().unwrap() {
        assert_eq!(row["signed_up"], 0, "{surface}");
        assert_eq!(row["claimed"], 0, "{surface}");
        assert_eq!(row["published"], 0, "{surface}");
        assert_eq!(row["first_external_call"], 0, "{surface}");
        assert!(row["day7_active"].is_null(), "{surface}");
    }

    tenant(&db, "named", "SynthOrg-run-7", "external", "external").await;
    tenant(&db, "classed", "cursor", "fleet", "external").await;
    tenant(&db, "looped", "cursor", "loopback", "external").await;
    tenant(&db, "origined", "cursor", "external", "synthetic").await;
    tenant(&db, "ordinary", "cursor", "external", "external").await;

    let (by_surface, _, excluded) = mcphost::funnel::surface_report(&db, now, 7).await.expect("report");
    assert_eq!(excluded, 4);
    assert_eq!(by_surface["cursor"]["signed_up"], 1);
    assert_eq!(total_signed_up(&by_surface), 1, "synthetic tenants sit in no row, `other` included");
}
