//! PRD-mcphost-admin-funnel-real-surface-recipe-counter
//! AC2 (P0) — Given a human tenant with a successful call as itself, one
//! whose successful call has `caller_tenant_id` NULL and `end_user_subject`
//! NULL, one whose only successful call came from another tenant, and one
//! with no call, When `surface_report` runs, Then `first_own_call` is 1 for
//! the first and the NULL-caller tenant only, `first_external_call` is 1 for
//! the other-tenant one only, and a call before `created_unix` counts for
//! neither.

use crate::common::TempDataDir;
use mcphost::auth::{generate_key, generate_namespace, hash_key};
use mcphost::db::Db;

const DAY: i64 = 86_400;

async fn tenant(db: &Db, name: &str, client: &str, born: i64) -> i64 {
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
    db.set_tenant_stamp_for_test(t.id, "created_unix", born).await.expect("backdate");
    t.id
}

async fn call(db: &Db, tenant_id: i64, caller: Option<i64>, ok: bool, at: i64) {
    db.insert_call_row_for_test_full(tenant_id, "t".into(), caller, None, ok, 1, at).await.expect("call");
}

#[tokio::test]
async fn own_and_external_cells_are_complements_and_ignore_calls_before_signup() {
    let dir = TempDataDir::new();
    let db = Db::open(&dir.0).expect("open db");
    db.migrate().await.expect("migrate");
    let now = mcphost::state::now_unix();
    let born = now - 2 * DAY;

    // One tenant per surface so each cell can be read in isolation.
    let own = tenant(&db, "own", "cursor", born).await;
    call(&db, own, Some(own), true, born + 60).await;
    let null_caller = tenant(&db, "null", "claude_code", born).await;
    call(&db, null_caller, None, true, born + 60).await;
    let other = tenant(&db, "other", "windsurf", born).await;
    call(&db, other, Some(own), true, born + 60).await;
    let _none = tenant(&db, "none", "goose", born).await;
    // Successful calls before `created_unix` count for neither cell, and a
    // failed call after it counts for neither either.
    let early = tenant(&db, "early", "cline", born).await;
    call(&db, early, None, true, born - 3600).await;
    call(&db, early, Some(own), true, born - 1800).await;
    call(&db, early, None, false, born + 60).await;

    let (by_surface, _, excluded) = mcphost::funnel::surface_report(&db, now, 7).await.expect("report");
    assert_eq!(excluded, 0);
    let cell = |surface: &str, key: &str| by_surface[surface][key].as_i64().unwrap();
    assert_eq!((cell("cursor", "first_own_call"), cell("cursor", "first_external_call")), (1, 0));
    assert_eq!((cell("claude_code", "first_own_call"), cell("claude_code", "first_external_call")), (1, 0));
    assert_eq!((cell("windsurf", "first_own_call"), cell("windsurf", "first_external_call")), (0, 1));
    assert_eq!((cell("goose", "first_own_call"), cell("goose", "first_external_call")), (0, 0));
    assert_eq!((cell("cline", "first_own_call"), cell("cline", "first_external_call")), (0, 0));
}
