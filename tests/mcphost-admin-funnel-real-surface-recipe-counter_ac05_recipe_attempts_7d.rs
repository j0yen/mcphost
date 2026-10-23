//! PRD-mcphost-admin-funnel-real-surface-recipe-counter
//! AC5 (P0) — Given recorded uses by an ordinary tenant and by a `synthorg`
//! tenant, When `admin.funnel` runs, Then `recipe_attempts_7d` counts only the
//! ordinary tenant's uses from the last 7 days (zero-filled per recipe) and
//! `recipe_attempts_synthetic_7d` equals the synthetic tenant's uses.

use crate::common;
use mcphost::auth::{generate_key, generate_namespace, hash_key};
use mcphost::db::Db;
use mcphost::kinds::aliases::recipe_names;
use serde_json::json;

const DAY: i64 = 86_400;

async fn tenant(db: &Db, name: &str, client: &str) -> i64 {
    db.create_tenant_attributed_with_source(
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
    .expect("create tenant")
    .id
}

#[tokio::test]
async fn recipe_attempts_count_ordinary_tenants_in_window_and_total_synthetic_apart() {
    let (state, _dir) = common::bare_app_state().await;
    let db = &state.db;
    let now = mcphost::state::now_unix();
    let names = recipe_names();
    let (first, second) = (names[0], names[1]);

    let ordinary = tenant(db, "ordinary", "cursor").await;
    db.insert_recipe_use(ordinary, first.to_string(), "quickstart", now - DAY).await.unwrap();
    db.insert_recipe_use(ordinary, first.to_string(), "publish", now - 2 * DAY).await.unwrap();
    db.insert_recipe_use(ordinary, second.to_string(), "quickstart", now - 6 * DAY).await.unwrap();
    // Outside the trailing 7 days: counts for nobody.
    let stale = tenant(db, "stale", "cursor").await;
    db.insert_recipe_use(stale, second.to_string(), "quickstart", now - 8 * DAY).await.unwrap();
    let synthetic = tenant(db, "synthetic", "synthorg-panel").await;
    for via in ["quickstart", "publish", "quickstart"] {
        db.insert_recipe_use(synthetic, first.to_string(), via, now - 3600).await.unwrap();
    }

    let admin = mcphost::admin::funnel(&state, &json!({"days": 7})).await.expect("admin.funnel");
    let attempts = admin["recipe_attempts_7d"].as_object().expect("recipe_attempts_7d");
    assert_eq!(attempts.len(), names.len(), "{admin}");
    for name in &names {
        let want = match *name {
            n if n == first => 2,
            n if n == second => 1,
            _ => 0,
        };
        assert_eq!(attempts[*name], json!(want), "{name}: {admin}");
    }
    assert_eq!(admin["recipe_attempts_synthetic_7d"], json!(3), "{admin}");
}
