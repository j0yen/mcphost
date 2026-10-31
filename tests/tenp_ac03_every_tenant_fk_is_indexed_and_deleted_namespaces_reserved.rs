//! PRD-mcphost-synthetic-tenant-purge
//! AC3 (P0) — Given the migrated schema, When a test lists every foreign-key
//! column referencing tenants through pragma_foreign_key_list, Then each has
//! an index whose first column it is, the migration adding the missing ones
//! takes the next free number at land time and also creates
//! namespace_tombstones holding each tenant-delete admin_events namespace, and
//! dropping any one of those indexes inside the test fails it naming the table
//! and column.

use crate::common::TestServer;
use rusqlite::Connection;
use std::collections::BTreeSet;

const MIGRATION: &str = include_str!("../migrations/0138_tenant_fk_indexes.sql");

/// `(table, column)` of every foreign key to `tenants` with no index whose first column it is.
fn uncovered(conn: &Connection) -> Vec<(String, String)> {
    let mut stmt = conn
        .prepare(
            "SELECT m.name, f.\"from\" FROM sqlite_master m, pragma_foreign_key_list(m.name) f \
             WHERE m.type = 'table' AND f.\"table\" = 'tenants' \
             AND NOT EXISTS (SELECT 1 FROM pragma_index_list(m.name) il, pragma_index_info(il.name) ii \
                             WHERE ii.seqno = 0 AND ii.name = f.\"from\") \
             ORDER BY m.name, f.\"from\"",
        )
        .expect("prepare");
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).expect("query").collect::<Result<_, _>>().expect("rows")
}

fn fk_columns(conn: &Connection) -> BTreeSet<(String, String)> {
    conn.prepare(
        "SELECT m.name, f.\"from\" FROM sqlite_master m, pragma_foreign_key_list(m.name) f \
         WHERE m.type = 'table' AND f.\"table\" = 'tenants'",
    )
    .expect("prepare")
    .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
    .expect("query")
    .collect::<Result<_, _>>()
    .expect("rows")
}

fn describe(missing: &[(String, String)]) -> String {
    missing.iter().map(|(t, c)| format!("{t}.{c}")).collect::<Vec<_>>().join(", ")
}

#[tokio::test]
async fn tenp_ac03_every_foreign_key_to_tenants_has_a_leading_index() {
    let server = TestServer::start().await;
    let conn = Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let all = fk_columns(&conn);
    assert!(all.len() > 100, "the enumeration found only {} foreign-key columns", all.len());
    let missing = uncovered(&conn);
    assert!(missing.is_empty(), "foreign-key columns to tenants with no leading index: {}", describe(&missing));

    // Dropping any index the migration added makes the check fail, naming that table and column.
    let added: Vec<(String, String, String)> = conn
        .prepare(
            "SELECT il.name, m.name, ii.name FROM sqlite_master m, pragma_index_list(m.name) il, \
             pragma_index_info(il.name) ii WHERE m.type = 'table' AND il.name LIKE 'idx\\_tenp\\_%' ESCAPE '\\' \
             AND ii.seqno = 0",
        )
        .expect("prepare")
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows");
    let on_fk: Vec<_> = added.iter().filter(|(_, t, c)| all.contains(&(t.clone(), c.clone()))).collect();
    assert!(on_fk.len() >= 26, "the migration indexes the 26 uncovered foreign keys, found {}", on_fk.len());
    for (index, table, column) in on_fk {
        conn.execute_batch("BEGIN").expect("begin");
        conn.execute_batch(&format!("DROP INDEX \"{index}\"")).expect("drop index");
        let missing = uncovered(&conn);
        conn.execute_batch("ROLLBACK").expect("rollback");
        assert_eq!(
            missing,
            vec![(table.clone(), column.clone())],
            "dropping {index} must fail the check naming {table}.{column}"
        );
    }
    assert!(uncovered(&conn).is_empty(), "every drop was rolled back");
}

/// The migration's own `INSERT OR IGNORE INTO namespace_tombstones` statement.
fn backfill_sql() -> &'static str {
    let start = MIGRATION.find("INSERT OR IGNORE INTO namespace_tombstones").expect("backfill statement");
    let end = start + MIGRATION[start..].find(';').expect("statement end") + 1;
    &MIGRATION[start..end]
}

#[tokio::test]
async fn tenp_ac03_namespace_tombstones_hold_each_tenant_delete_namespace() {
    let server = TestServer::start().await;
    let conn = Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let columns: Vec<(String, String, bool)> = conn
        .prepare("SELECT name, type, pk FROM pragma_table_info('namespace_tombstones') ORDER BY cid")
        .expect("prepare")
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? > 0)))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows");
    assert_eq!(
        columns,
        vec![
            ("namespace".to_string(), "TEXT".to_string(), true),
            ("purged_unix".to_string(), "INTEGER".to_string(), false),
            ("action".to_string(), "TEXT".to_string(), false),
        ]
    );

    // `migrate()` ran at open, before any admin_events row existed: plant them now and re-run the backfill.
    let planted = [
        ("tenp-gone-hand", "tenant_delete", "2026-09-01T10:00:00Z"),
        ("tenp-gone-external", "tenant_purge_expired", "2026-09-02T10:00:00Z"),
        ("tenp-gone-twice", "tenant_delete", "2026-09-03T10:00:00Z"),
        ("tenp-gone-twice", "tenant_purge_expired", "2026-09-04T10:00:00Z"),
        ("tenp-not-a-delete", "tenant_set_synthetic", "2026-09-05T10:00:00Z"),
    ];
    for (ns, action, ts) in planted {
        conn.execute("INSERT INTO admin_events (ts, action, tenant, detail) VALUES (?1, ?2, ?3, '{}')", [ts, action, ns])
            .expect("plant admin_events");
    }
    assert_eq!(conn.execute(backfill_sql(), []).expect("backfill"), 3, "one tombstone per deleted namespace");
    assert_eq!(conn.execute(backfill_sql(), []).expect("backfill again"), 0, "idempotent");
    let rows: Vec<(String, String, i64)> = conn
        .prepare("SELECT namespace, action, purged_unix FROM namespace_tombstones WHERE namespace LIKE 'tenp-%' ORDER BY namespace")
        .expect("prepare")
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows");
    let unix = |ts: &str| -> i64 {
        conn.query_row("SELECT CAST(strftime('%s', ?1) AS INTEGER)", [ts], |r| r.get(0)).expect("unix")
    };
    assert_eq!(
        rows,
        vec![
            ("tenp-gone-external".to_string(), "tenant_purge_expired".to_string(), unix("2026-09-02T10:00:00Z")),
            ("tenp-gone-hand".to_string(), "tenant_delete".to_string(), unix("2026-09-01T10:00:00Z")),
            ("tenp-gone-twice".to_string(), "tenant_delete".to_string(), unix("2026-09-03T10:00:00Z")),
        ]
    );
}
