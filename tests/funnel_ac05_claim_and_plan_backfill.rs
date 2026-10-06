//! PRD-mcphost-activation-funnel
//! AC5 (P0) — Given a tenant that was claimed before this release, When
//! the migration runs, Then its `claimed_unix` equals its
//! `owner_verified_at`; given a Pro tenant, Then `paid_unix` equals its
//! `plan_since`.
//!
//! `Db::open`/`migrate()` always runs every migration, so this test can't
//! literally catch the crate mid-migration -- same reasoning
//! `attrib_ac3_backfill_migration.rs` already documents. It instead
//! exercises the exact function migration 0072's own gate calls,
//! `Db::backfill_activation_claims_and_plans`, against rows whose
//! `owner_verified_at`/`plan_since` are seeded directly (over a raw
//! connection) while `claimed_unix`/`paid_unix` are left `NULL` -- exactly
//! the shape a genuinely pre-0072 row would be in the instant its own
//! `ALTER TABLE`s land.

use crate::common;
use common::TempDataDir;
use mcphost::auth::{generate_key, generate_namespace, hash_key};
use mcphost::db::Db;
use mcphost::state::{now_unix, rfc3339_from_unix};

#[tokio::test]
async fn backfill_sets_claimed_unix_and_paid_unix_from_pre_release_columns() {
    let dir = TempDataDir::new();
    let db = Db::open(&dir.0).expect("open db");
    db.migrate().await.expect("migrate");

    let claimed_ns = generate_namespace();
    let claimed_tenant = db
        .create_tenant(
            "AC5 Claimed Tenant".to_string(),
            claimed_ns.clone(),
            hash_key(&generate_key()),
            None,
        )
        .await
        .expect("seed claimed tenant");

    let pro_ns = generate_namespace();
    let pro_tenant = db
        .create_tenant("AC5 Pro Tenant".to_string(), pro_ns.clone(), hash_key(&generate_key()), None)
        .await
        .expect("seed pro tenant");

    let owner_verified_at = now_unix() - 100_000;
    let plan_since_unix = now_unix() - 50_000;
    let plan_since = rfc3339_from_unix(plan_since_unix);

    let conn = rusqlite::Connection::open(dir.0.join("mcphost.db")).expect("open raw db");
    conn.execute(
        "UPDATE tenants SET owner_email = 'owner@example.com', owner_verified_at = ?1 WHERE id = ?2",
        rusqlite::params![owner_verified_at, claimed_tenant.id],
    )
    .expect("seed owner_verified_at");
    conn.execute(
        "UPDATE tenants SET plan = 'pro', plan_since = ?1 WHERE id = ?2",
        rusqlite::params![plan_since, pro_tenant.id],
    )
    .expect("seed pro plan_since");
    drop(conn);

    // Neither tenant has its new stamp yet -- the seeded rows really do
    // mimic a pre-0072 row, not one the live write path already caught.
    let before = db.find_tenant_by_namespace(claimed_ns.clone()).await.unwrap().unwrap();
    assert!(before.claimed_unix.is_none(), "{before:?}");
    let before_pro = db.find_tenant_by_namespace(pro_ns.clone()).await.unwrap().unwrap();
    assert!(before_pro.paid_unix.is_none(), "{before_pro:?}");

    let (claimed_touched, paid_touched) =
        db.backfill_activation_claims_and_plans().await.expect("backfill");
    assert_eq!(claimed_touched, 1, "exactly the one pre-release claimed tenant");
    assert_eq!(paid_touched, 1, "exactly the one pre-release pro tenant");

    let claimed_after = db.find_tenant_by_namespace(claimed_ns).await.unwrap().unwrap();
    assert_eq!(claimed_after.claimed_unix, Some(owner_verified_at), "{claimed_after:?}");

    let pro_after = db.find_tenant_by_namespace(pro_ns).await.unwrap().unwrap();
    assert_eq!(pro_after.paid_unix, Some(plan_since_unix), "{pro_after:?}");

    // Re-running the backfill (mirroring `migrate()`'s own idempotency
    // contract) must be a no-op: nothing is left to touch.
    let (claimed_again, paid_again) =
        db.backfill_activation_claims_and_plans().await.expect("re-run backfill");
    assert_eq!(claimed_again, 0);
    assert_eq!(paid_again, 0);
}
