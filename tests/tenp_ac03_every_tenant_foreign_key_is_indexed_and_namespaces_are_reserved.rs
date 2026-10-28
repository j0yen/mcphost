//! PRD-mcphost-synthetic-tenant-purge
//! AC3 (P0) — Given the migrated schema, When a test lists every foreign-key column referencing tenants
//! through pragma_foreign_key_list, Then each has an index whose first column it is, the migration
//! adding the missing ones takes the next free number at land time and also creates
//! namespace_tombstones holding each tenant-delete admin_events namespace, and dropping any one of
//! those indexes inside the test fails it naming the table and column.
//! contract: every foreign-key column to tenants has a leading index, and the migration that adds them also backfills namespace_tombstones from the tenant-delete admin_events rows

use crate::common::tenp;
use crate::common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use rusqlite::{Connection, params};
use serde_json::json;
use std::collections::BTreeSet;

/// The foreign-key columns to `tenants` that no index leads with, as `table.column`.
fn uncovered(conn: &Connection) -> Vec<String> {
    let mut out = Vec::new();
    for fk in tenp::tenant_fk_cols(conn) {
        let indexes: Vec<String> = conn
            .prepare("SELECT name FROM pragma_index_list(?1)")
            .unwrap()
            .query_map(params![fk.table], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let led = indexes.iter().any(|ix| {
            conn.query_row("SELECT name FROM pragma_index_info(?1) WHERE seqno = 0", params![ix], |r| r.get::<_, String>(0))
                .is_ok_and(|first| first == fk.column)
        });
        if !led {
            out.push(format!("{}.{}", fk.table, fk.column));
        }
    }
    out
}

const MIGRATIONS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/migrations");

/// `(file name, text)` of the migration that creates `namespace_tombstones`.
fn tombstone_migration() -> (String, String) {
    let found: Vec<(String, String)> = std::fs::read_dir(MIGRATIONS)
        .unwrap()
        .flatten()
        .map(|e| (e.file_name().to_string_lossy().to_string(), std::fs::read_to_string(e.path()).unwrap()))
        .filter(|(_, text)| text.contains("CREATE TABLE IF NOT EXISTS namespace_tombstones"))
        .collect();
    assert_eq!(found.len(), 1, "exactly one migration creates namespace_tombstones: {:?}", found.iter().map(|f| &f.0).collect::<Vec<_>>());
    found.into_iter().next().unwrap()
}

#[tokio::test]
async fn tenp_ac03_every_foreign_key_column_to_tenants_has_a_leading_index() {
    let server = TestServer::start().await;
    let conn = Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let columns = tenp::tenant_fk_cols(&conn);
    assert!(columns.len() >= 118, "the enumeration sees the whole schema: {}", columns.len());
    assert_eq!(uncovered(&conn), Vec::<String>::new(), "foreign-key columns to tenants without a leading index");

    // The columns the purge looks up by value, with no foreign key behind them.
    for index in ["idx_runs_caller_tenant_id", "idx_usage_daily_caller_tenant_id", "idx_invites_org_id", "idx_tenants_scratch_of"] {
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?1", params![index], |r| r.get(0)).unwrap();
        assert_eq!(n, 1, "{index} exists");
    }
}

#[tokio::test]
async fn tenp_ac03_dropping_any_migration_added_index_fails_the_check_naming_table_and_column() {
    let server = TestServer::start().await;
    let conn = Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let (_, sql) = tombstone_migration();
    // `CREATE INDEX IF NOT EXISTS <name> ON <table>(<column>)` lines whose column is a tenant foreign key.
    let fks: BTreeSet<String> = tenp::tenant_fk_cols(&conn).into_iter().map(|f| format!("{}.{}", f.table, f.column)).collect();
    let mut dropped = 0;
    for line in sql.lines().filter(|l| l.starts_with("CREATE INDEX IF NOT EXISTS")) {
        let words: Vec<&str> = line.split_whitespace().collect();
        let name = words[5];
        let target = words[7];
        let (table, rest) = target.split_once('(').expect("ON table(column)");
        let column = rest.trim_end_matches(';').trim_end_matches(')');
        let key = format!("{table}.{column}");
        if !fks.contains(&key) {
            continue;
        }
        conn.execute_batch("BEGIN").unwrap();
        conn.execute_batch(&format!("DROP INDEX {name}")).unwrap();
        let missing = uncovered(&conn);
        conn.execute_batch("ROLLBACK").unwrap();
        assert_eq!(missing, vec![key.clone()], "dropping {name} fails the check, naming {key}");
        dropped += 1;
    }
    assert!(dropped >= 26, "every index the migration adds for a foreign key was dropped in turn: {dropped}");
    assert_eq!(uncovered(&conn), Vec::<String>::new(), "each drop was rolled back");
}

#[tokio::test]
async fn tenp_ac03_the_migration_takes_the_next_free_number_and_backfills_every_tenant_delete_namespace() {
    // This migration's number is its own (no other file shares it) and the numbers below it have no
    // gap, i.e. it is the next free one. (0113 predates this PRD and exists twice; nothing here
    // asserts on it.)
    let (file, sql) = tombstone_migration();
    let own: u32 = file[..4].parse().expect("NNNN_name.sql");
    let numbers: Vec<u32> = std::fs::read_dir(MIGRATIONS)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy()[..4].parse().expect("NNNN_name.sql"))
        .collect();
    assert_eq!(numbers.iter().filter(|n| **n == own).count(), 1, "{file}: the number is taken by no other migration");
    let below: BTreeSet<u32> = numbers.iter().copied().filter(|n| *n < own).collect();
    assert_eq!(below, (1..own).collect::<BTreeSet<_>>(), "{file}: no number below it is free");

    let server = TestServer::start().await;
    let conn = Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let columns: Vec<(String, i64)> = conn
        .prepare("SELECT name, pk FROM pragma_table_info('namespace_tombstones') ORDER BY cid")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(columns, vec![("namespace".to_string(), 1), ("purged_unix".to_string(), 0), ("action".to_string(), 0)]);

    // `migrate()` ran at open, before any delete: plant the delete journal after open, then re-run the
    // migration's own backfill statement (read from the file, never retyped here).
    let start = sql.find("INSERT OR IGNORE INTO namespace_tombstones").expect("the backfill statement");
    let backfill = &sql[start..start + sql[start..].find(';').expect("statement ends") + 1];
    for (ts, action, tenant) in [
        ("2026-10-01T00:00:00Z", "tenant_delete", "tenp-gone-by-hand"),
        ("2026-10-02T00:00:00Z", "tenant_purge_expired", "tenp-gone-by-expiry"),
        ("2026-10-03T00:00:00Z", "tenant_disable", "tenp-only-disabled"),
        ("2026-10-04T00:00:00Z", "implicit_signup_rolled_back", "tenp-rolled-back"),
    ] {
        conn.execute("INSERT INTO admin_events (ts, action, tenant, detail) VALUES (?1, ?2, ?3, '{}')", params![ts, action, tenant]).unwrap();
    }
    conn.execute_batch(backfill).unwrap();
    conn.execute_batch(backfill).unwrap(); // idempotent
    let rows: Vec<(String, String)> = conn
        .prepare("SELECT namespace, action FROM namespace_tombstones WHERE namespace LIKE 'tenp-%' ORDER BY namespace")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        rows,
        vec![
            ("tenp-gone-by-expiry".to_string(), "tenant_purge_expired".to_string()),
            ("tenp-gone-by-hand".to_string(), "tenant_delete".to_string()),
        ],
        "one tombstone per tenant-delete namespace, with its action; no other action is a delete"
    );
    let purged: i64 = conn
        .query_row("SELECT purged_unix FROM namespace_tombstones WHERE namespace = 'tenp-gone-by-hand'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(purged, 1_790_812_800, "purged_unix is the journal row's own time (2026-10-01T00:00:00Z)");

    // A live delete writes its tombstone as well.
    let (ns, _key) = signup(&server.base_url, "AC3 deleted by hand").await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let out = extract_structured(&admin.tools_call("admin.tenant_delete", json!({"tenant": ns})).await.expect("tenant_delete"));
    assert_eq!(out["namespace"], ns, "{out}");
    let action: String = conn.query_row("SELECT action FROM namespace_tombstones WHERE namespace = ?1", params![ns], |r| r.get(0)).expect("a tombstone");
    assert_eq!(action, "tenant_delete");
}
