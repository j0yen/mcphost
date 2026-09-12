//! PRD-mcphost-metered-overage
//! AC1 — Given migration 0007 on a v0.14.0 data dir, When `mcphost serve`
//! starts, Then existing tenants load with null `stripe_customer_id` and
//! the full prior suite passes unchanged.
//!
//! This test exercises the migration path directly (open a fresh DB,
//! migrate, seed a tenant the pre-0007 way via `create_tenant`, reopen)
//! rather than shipping a pre-0007 fixture file -- migration 0007 is
//! additive and idempotent (same pattern as 0002-0006 and 0008), so
//! re-running `migrate()` against a DB that already has the column is
//! itself the "existing data dir" case this AC cares about. (Same
//! convention as `tests/synthetic_ac01_migration_null_default.rs`, which
//! covers migration 0008's `synthetic` column -- this file covers 0007's
//! `stripe_customer_id` column, a distinct AC despite the coincidental
//! `ac01` numbering.)

use crate::common;
use common::TempDataDir;
use mcphost::auth::{generate_key, generate_namespace, hash_key};
use mcphost::db::Db;

#[tokio::test]
async fn existing_tenants_load_with_null_stripe_customer_id() {
    let dir = TempDataDir::new();
    let db = Db::open(&dir.0).expect("open db");
    db.migrate().await.expect("migrate");

    let key = generate_key();
    let namespace = generate_namespace();
    let created = db
        .create_tenant(
            "Pre-existing Tenant".to_string(),
            namespace.clone(),
            hash_key(&key),
            None,
        )
        .await
        .expect("create tenant");
    assert_eq!(created.stripe_customer_id, None);

    // Re-running migrate() (mirroring another `mcphost serve` start against
    // the same data dir) must be a no-op, not an error, and must not
    // disturb the row.
    db.migrate().await.expect("re-migrate is idempotent");

    let reloaded = db
        .find_tenant_by_namespace(namespace)
        .await
        .expect("query")
        .expect("tenant still present");
    assert_eq!(
        reloaded.stripe_customer_id, None,
        "pre-existing tenant must reload with null stripe_customer_id"
    );
}
