//! PRD-mcphost-funnel-by-surface-nightly-probe-b
//! AC1 (P0) — Given a call on the tenant's own tool made with the tenant's
//! own key, When `first_external_call` is computed, Then that call does not
//! count; a call from another tenant's key or an anonymous public-URL call
//! does.

use crate::common::TempDataDir;
use mcphost::auth::{generate_key, generate_namespace, hash_key};
use mcphost::db::Db;

const DAY: i64 = 86_400;

async fn human(db: &Db, name: &str, born: i64) -> i64 {
    let t = db
        .create_tenant_attributed_with_source(
            name.to_string(),
            generate_namespace(),
            hash_key(&generate_key()),
            None,
            Some("external".to_string()),
            Some("cursor".to_string()),
            None,
            "external".to_string(),
            None,
            None,
            "human".to_string(),
        )
        .await
        .expect("create tenant");
    db.set_tenant_stamp_for_test(t.id, "created_unix", born).await.expect("backdate");
    t.id
}

#[tokio::test]
async fn only_foreign_key_or_anonymous_calls_count_as_first_external_call() {
    let dir = TempDataDir::new();
    let db = Db::open(&dir.0).expect("open db");
    db.migrate().await.expect("migrate");
    let now = mcphost::state::now_unix();
    let born = now - 3 * DAY;

    let own_only = human(&db, "own-only", born).await;
    let foreign = human(&db, "foreign", born).await;
    let anon = human(&db, "anon", born).await;
    let other = human(&db, "other-caller", born).await;

    // Own key: no caller tenant, no end user.
    db.insert_call_row_for_test_full(own_only, "t".into(), None, None, true, 1, born + DAY).await.unwrap();
    // Another tenant's key.
    db.insert_call_row_for_test_full(foreign, "t".into(), Some(other), None, true, 1, born + DAY).await.unwrap();
    // Anonymous public-URL caller (end user, no tenant key).
    db.insert_call_row_for_test_full(anon, "t".into(), None, Some("visitor".into()), true, 1, born + DAY)
        .await
        .unwrap();

    let (by_surface, _probe, _excluded) = mcphost::funnel::surface_report(&db, now, 7).await.expect("report");
    // own_only does not count; foreign and anon do.
    assert_eq!(by_surface["cursor"]["signed_up"], 4, "{by_surface}");
    assert_eq!(by_surface["cursor"]["first_external_call"], 2, "{by_surface}");
}
