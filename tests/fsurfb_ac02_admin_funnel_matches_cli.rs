//! PRD-mcphost-funnel-by-surface-nightly-probe-b
//! AC2 (P0) — Given `admin.funnel` with `days: 7`, When called with an admin
//! key, Then the response carries `by_surface` and `probe` with the same
//! values as `mcphost funnel --json --days 7` on the same DB.

use crate::common;
use mcphost::auth::{generate_key, generate_namespace, hash_key};
use mcphost::db::Db;
use serde_json::{Value, json};

const DAY: i64 = 86_400;

async fn tenant(db: &Db, name: &str, client: &str, origin: &str, born: i64) -> i64 {
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
            origin.to_string(),
        )
        .await
        .expect("create tenant");
    db.set_tenant_stamp_for_test(t.id, "created_unix", born).await.expect("backdate");
    t.id
}

#[tokio::test]
async fn admin_funnel_by_surface_and_probe_equal_the_cli_json() {
    let (state, dir) = common::bare_app_state().await;
    let now = mcphost::state::now_unix();
    let a = tenant(&state.db, "A", "cursor", "human", now - 3 * DAY).await;
    let _b = tenant(&state.db, "B", "claude-code", "human", now - 2 * DAY).await;
    let _c = tenant(&state.db, "C", "weird-client", "human", now - DAY).await;
    let f = tenant(&state.db, "journey", "claude-code", "fleet", now - 2 * 3600).await;
    state
        .db
        .insert_call_row_for_test_full(f, "t".into(), Some(a), None, true, 1, now - 3600)
        .await
        .expect("fleet external call");

    let admin = mcphost::admin::funnel(&state, &json!({"days": 7})).await.expect("admin.funnel");

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["funnel", "--json", "--days", "7"])
        .env("MCPHOST_DATA_DIR", &dir.0)
        .output()
        .expect("spawn mcphost funnel");
    assert!(output.status.success(), "stderr={}", String::from_utf8_lossy(&output.stderr));
    let cli: Value = serde_json::from_slice(&output.stdout).expect("--json parses");

    assert_eq!(admin["by_surface"], cli["by_surface"], "admin={admin} cli={cli}");
    assert_eq!(admin["probe"], cli["probe"], "admin={admin} cli={cli}");
    assert_eq!(admin["by_surface"]["cursor"]["signed_up"], json!(1), "{admin}");
    assert_eq!(admin["by_surface"]["other"]["signed_up"], json!(1), "{admin}");
    assert_eq!(admin["probe"].as_array().map(Vec::len), Some(1), "{admin}");
}
