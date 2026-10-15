//! PRD-mcphost-heal-action-registry-and-ledger-b AC2 (P0): auto incidents
//! written before the 0096 ledger columns existed read back with
//! `action: null`, and `heal.last_24h` counts only rows with an action.

use crate::common;

use mcphost::heal::{HealAction, HealOutcome, HealRow};
use serde_json::json;

#[tokio::test]
async fn healb_ac02_pre_ledger_auto_incidents_render_action_null() {
    let (state, dir) = common::bare_app_state().await;
    let now = mcphost::state::now_unix();

    // Rewind the schema to a 0.107.0 database: no ledger columns, three
    // open auto incidents, then let the real migration run again.
    {
        let conn = rusqlite::Connection::open(dir.0.join("mcphost.db")).expect("raw open");
        for col in ["action", "trigger_json", "before_json", "after_json", "outcome", "dry_run", "duration_ms"] {
            conn.execute_batch(&format!("ALTER TABLE incidents DROP COLUMN {col}")).expect("drop column");
        }
        for i in 0..3 {
            conn.execute(
                "INSERT INTO incidents (title, impact, components_json, opened_at, closed_at, timeline_json, auto) \
                 VALUES (?1, 'partial', '[]', ?2, NULL, '[]', 1)",
                rusqlite::params![format!("old {i}"), now - 60],
            )
            .expect("insert old row");
        }
    }
    state.db.migrate().await.expect("re-migrate");

    let row = HealRow {
        id: 0,
        action: HealAction::LogRotate,
        at: now,
        outcome: HealOutcome::Ok,
        dry_run: false,
        duration_ms: 1,
        trigger: json!({}),
        before: json!({}),
        after: json!({}),
    };
    state.db.insert_heal_row(row).await.expect("heal row");

    let status = mcphost::statusfeed::status_json(&state).await.expect("status_json");
    let open = status["incidents_open"].as_array().expect("incidents_open");
    let old: Vec<_> = open.iter().filter(|i| i["title"].as_str().is_some_and(|t| t.starts_with("old "))).collect();
    assert_eq!(old.len(), 3);
    for i in old {
        assert!(i["action"].is_null(), "{i}");
        assert!(i["outcome"].is_null());
    }
    let last = &status["heal"]["last_24h"];
    assert_eq!(last["ok"], json!(1));
    let total: i64 = last.as_object().expect("last_24h").values().map(|v| v.as_i64().unwrap_or(0)).sum();
    assert_eq!(total, 1, "pre-ledger rows are not counted: {last}");
}
