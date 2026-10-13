//! PRD-mcphost-funnel-by-surface-nightly-probe
//! AC3 (P0) — Given a planted DB with 3 humans on `cursor` (1 published, 1
//! with an external call on day 2 and a call on day 9) and 1 fleet tenant
//! born 2 h ago that reached `first_external_call`, When `mcphost funnel
//! --json --days 14` runs, Then `by_surface.cursor` reads `signed_up 3,
//! published 1, first_external_call 1, day7_active 1` and `probe` holds
//! exactly the fleet tenant at stage `first_external_call`.

use crate::common::TempDataDir;
use mcphost::auth::{generate_key, generate_namespace, hash_key};
use mcphost::db::{Db, Tenant};
use serde_json::{Value, json};

const DAY: i64 = 86_400;

async fn tenant(db: &Db, name: &str, client: &str, funnel_origin: &str, created_unix: i64) -> Tenant {
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
            funnel_origin.to_string(),
        )
        .await
        .expect("create tenant");
    db.set_tenant_stamp_for_test(t.id, "created_unix", created_unix).await.expect("backdate");
    t
}

#[tokio::test]
async fn cursor_row_counts_stages_and_probe_names_the_fleet_tenant() {
    let dir = TempDataDir::new();
    let now = mcphost::state::now_unix();
    {
        let db = Db::open(&dir.0).expect("open db");
        db.migrate().await.expect("migrate");

        // Three human cursor tenants, each 14 days old (day7_active is
        // measurable for all of them).
        let born = now - 14 * DAY;
        let a = tenant(&db, "A", "cursor", "human", born).await;
        let b = tenant(&db, "B", "cursor", "human", born).await;
        let _c = tenant(&db, "C", "Cursor", "human", born).await;

        // B publishes a tool.
        db.upsert_tool(b.id, "greet".to_string(), "echo".to_string(), json!({}), 5, vec![])
            .await
            .expect("publish");
        // A: an external call (caller = B) on day 2, its own call on day 9.
        db.insert_call_row_for_test_full(a.id, "t".into(), Some(b.id), None, true, 1, born + 2 * DAY)
            .await
            .expect("external call");
        db.insert_call_row_for_test_full(a.id, "t".into(), None, None, true, 1, born + 9 * DAY)
            .await
            .expect("own call");

        // A fleet tenant born 2 h ago that got an external call.
        let fleet = tenant(&db, "journey", "claude-code", "fleet", now - 2 * 3600).await;
        db.insert_call_row_for_test_full(fleet.id, "t".into(), Some(a.id), None, true, 1, now - 3600)
            .await
            .expect("fleet external call");
    }

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["funnel", "--json", "--days", "14"])
        .env("MCPHOST_DATA_DIR", &dir.0)
        .output()
        .expect("spawn mcphost funnel");
    assert!(
        output.status.success(),
        "stderr={} stdout={}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let report: Value = serde_json::from_slice(&output.stdout).expect("--json parses");

    let cursor = &report["by_surface"]["cursor"];
    assert_eq!(cursor["signed_up"], json!(3), "{report}");
    assert_eq!(cursor["published"], json!(1), "{report}");
    assert_eq!(cursor["first_external_call"], json!(1), "{report}");
    assert_eq!(cursor["day7_active"], json!(1), "{report}");
    // The fleet tenant never counts as a human, and every id has a row.
    assert_eq!(report["by_surface"]["claude_code"]["signed_up"], json!(0), "{report}");
    for id in mcphost::install_links::SURFACES.iter().chain(["other"].iter()) {
        assert!(report["by_surface"].get(*id).is_some(), "missing row {id}: {report}");
    }

    let probe = report["probe"].as_array().expect("probe array");
    assert_eq!(probe.len(), 1, "{report}");
    assert_eq!(probe[0]["stage"], json!("first_external_call"), "{report}");
    assert_eq!(probe[0]["client_name"], json!("claude-code"), "{report}");
    assert_eq!(probe[0]["reached_at"], json!(now - 3600), "{report}");
}
