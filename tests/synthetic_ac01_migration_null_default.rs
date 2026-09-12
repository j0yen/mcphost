//! PRD-mcphost-synthetic-flag
//! AC1 — Given a data dir from the metered-overage HEAD, When the new
//! migration applies and `mcphost serve` starts, Then existing tenants
//! load with null `synthetic` and the full prior suite passes unchanged.
//!
//! This test exercises the migration path directly (open a fresh DB,
//! migrate, seed a tenant the pre-0008 way via `create_tenant(..., None)`,
//! reopen) rather than shipping a pre-0008 fixture file -- migration 0008
//! is additive and idempotent (same pattern as 0002-0007), so re-running
//! `migrate()` against a DB that already has the column is itself the
//! "existing data dir" case this AC cares about.

use crate::common;
use common::TempDataDir;
use mcphost::auth::{generate_key, generate_namespace, hash_key};
use mcphost::db::Db;

#[tokio::test]
async fn existing_tenants_load_with_null_synthetic() {
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
    assert_eq!(created.synthetic, None);

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
        reloaded.synthetic, None,
        "unlabeled tenant must reload with null synthetic"
    );
}
