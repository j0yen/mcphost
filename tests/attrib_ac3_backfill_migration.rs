//! PRD-mcphost-tenant-attribution
//! AC3 — Given the production DB state of 2026-09-08 (95 unlabeled
//! loopback tenants), When the backfill migration runs, Then all 95 are
//! classified synthetic (`classified_by = backfill`), `wintermute-hub` is
//! `fleet`, and `/healthz` reports `tenants_real = 0`.
//!
//! `Db::open` always runs every migration (this crate has no "stop at
//! migration N" hook, and shipping a raw pre-0010 fixture file would drift
//! the moment the schema changes again), so this test can't literally
//! catch the crate mid-migration. Instead it exercises the exact function
//! the migration's one-shot gate calls --
//! `Db::backfill_unclassified_tenants` (`db.rs`'s
//! `backfill_unclassified_tenants_sync`) -- against fixture rows seeded
//! with `source_class: None` via `create_tenant_attributed`, the same
//! state a genuinely pre-0010 row would be in the instant the `ALTER
//! TABLE` lands. `synthetic_ac01_migration_null_default.rs` established
//! this "exercise the migration path directly" pattern for 0008.

mod common;
use common::TempDataDir;
use mcphost::auth::{generate_key, generate_namespace, hash_key};
use mcphost::db::Db;

#[tokio::test]
async fn backfill_classifies_loopback_tenants_synthetic_and_hub_as_fleet() {
    let dir = TempDataDir::new();
    let db = Db::open(&dir.0).expect("open db");
    db.migrate().await.expect("migrate");

    // 95 unlabeled "panel persona" tenants (the PRD's exact production
    // scenario), plus the fleet's own tenant, all seeded as if they
    // predated migration 0010 (`source_class: None`).
    let mut unlabeled_namespaces = Vec::new();
    for i in 0..95 {
        let key = generate_key();
        let namespace = generate_namespace();
        db.create_tenant_attributed(
            format!("Panel Integration Specialist {i:02}"),
            namespace.clone(),
            hash_key(&key),
            None,
            None,
            None,
            None,
            "unclassified".to_string(),
            None,
        )
        .await
        .expect("seed unlabeled tenant");
        unlabeled_namespaces.push(namespace);
    }
    let hub_key = generate_key();
    let hub_namespace = generate_namespace();
    db.create_tenant_attributed(
        "wintermute-hub".to_string(),
        hub_namespace.clone(),
        hash_key(&hub_key),
        None,
        None,
        None,
        None,
        "unclassified".to_string(),
        None,
    )
    .await
    .expect("seed wintermute-hub");

    let touched = db
        .backfill_unclassified_tenants()
        .await
        .expect("backfill");
    assert_eq!(touched, 96, "95 personas + wintermute-hub");

    for ns in &unlabeled_namespaces {
        let tenant = db
            .find_tenant_by_namespace(ns.clone())
            .await
            .expect("query")
            .expect("tenant exists");
        assert_eq!(tenant.source_class.as_deref(), Some("loopback"), "{ns}");
        assert_eq!(
            tenant.synthetic.as_deref(),
            Some("harness:unstamped"),
            "{ns}"
        );
        assert_eq!(tenant.classified_by.as_deref(), Some("backfill"), "{ns}");
    }

    let hub = db
        .find_tenant_by_namespace(hub_namespace)
        .await
        .expect("query")
        .expect("hub exists");
    assert_eq!(hub.source_class.as_deref(), Some("fleet"));
    assert_eq!(hub.synthetic.as_deref(), Some("harness:unstamped"));
    assert_eq!(hub.classified_by.as_deref(), Some("backfill"));

    // AC3: `/healthz`'s tenants_real (source_class = external) is 0 --
    // nothing here was ever classified external.
    let real = db.count_external_tenants().await.expect("count");
    assert_eq!(real, 0);
    let (_tools_total, tenants_total) = db.counts().await.expect("counts");
    assert_eq!(tenants_total, 96);

    // Re-running the backfill (mirroring `migrate()`'s idempotency
    // contract) must be a no-op: nothing is left `source_class IS NULL`.
    let touched_again = db
        .backfill_unclassified_tenants()
        .await
        .expect("re-run backfill");
    assert_eq!(touched_again, 0);
}
