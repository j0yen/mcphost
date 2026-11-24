//! PRD-mcphost-synthetic-tenant-purge
//! contract: reads migrations/ for the file that creates namespace_tombstones and its backfill
//! AC3 (P0) — Given the migrated schema, When a test lists every foreign-key
//! column referencing `tenants` through `pragma_foreign_key_list`, Then each has
//! an index whose first column it is, the migration adding the missing ones takes
//! the next free number at land time and also creates `namespace_tombstones`
//! holding each tenant-delete `admin_events` namespace, and dropping any one of
//! those indexes inside the test fails it naming the table and column.

use crate::common::TempDataDir;
use crate::common::tenp;
use rusqlite::{Connection, params};
use std::collections::BTreeSet;
use std::path::Path;

/// Every foreign-key column that references `tenants` without an index whose
/// first column it is, as `table.column`.
fn uncovered_foreign_keys(conn: &Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare(
            "SELECT m.name, f.\"from\" FROM sqlite_master m, pragma_foreign_key_list(m.name) f \
             WHERE m.type = 'table' AND f.\"table\" = 'tenants' \
             AND NOT EXISTS (SELECT 1 FROM pragma_index_list(m.name) il, pragma_index_info(il.name) ii \
                             WHERE ii.seqno = 0 AND ii.name = f.\"from\") \
             ORDER BY 1, 2",
        )
        .unwrap();
    stmt.query_map([], |r| Ok(format!("{}.{}", r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

/// The check the test applies: `Err` names each uncovered table and column.
fn foreign_keys_covered(conn: &Connection) -> Result<usize, String> {
    let missing = uncovered_foreign_keys(conn);
    if !missing.is_empty() {
        return Err(format!("foreign-key column(s) referencing tenants without a leading index: {}", missing.join(", ")));
    }
    let total: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master m, pragma_foreign_key_list(m.name) f \
             WHERE m.type = 'table' AND f.\"table\" = 'tenants'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    Ok(total as usize)
}

/// The migration file that creates `namespace_tombstones`, whichever number it
/// lands as, and its text.
fn tombstone_migration() -> (String, String) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let mut found = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        if text.contains("CREATE TABLE IF NOT EXISTS namespace_tombstones") {
            found.push((path.file_name().unwrap().to_string_lossy().to_string(), text));
        }
    }
    assert_eq!(found.len(), 1, "exactly one migration creates namespace_tombstones: {:?}", found.iter().map(|f| &f.0).collect::<Vec<_>>());
    found.remove(0)
}

/// `(index name, table, column)` of every `CREATE INDEX IF NOT EXISTS name ON table(column)` in `sql`.
fn created_indexes(sql: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for stmt in sql.lines().filter(|l| !l.trim_start().starts_with("--")).collect::<Vec<_>>().join("\n").split(';') {
        let stmt = stmt.trim();
        let Some(rest) = stmt.strip_prefix("CREATE INDEX IF NOT EXISTS ") else { continue };
        let (name, rest) = rest.split_once(" ON ").expect("ON");
        let (table, rest) = rest.split_once('(').expect("(");
        let column = rest.split(')').next().unwrap().trim().to_string();
        out.push((name.trim().to_string(), table.trim().to_string(), column));
    }
    out
}

/// The `INSERT OR IGNORE INTO namespace_tombstones ...` statement of the migration, verbatim.
fn backfill_statement(sql: &str) -> String {
    let uncommented: String = sql.lines().filter(|l| !l.trim_start().starts_with("--")).collect::<Vec<_>>().join("\n");
    let start = uncommented.find("INSERT OR IGNORE INTO namespace_tombstones").expect("the backfill statement");
    let end = uncommented[start..].find(';').expect("terminated") + start;
    uncommented[start..=end].to_string()
}

#[test]
fn tenp_ac03_every_tenant_foreign_key_is_indexed_and_dropping_one_fails_by_name() {
    let dir = TempDataDir::new();
    drop(mcphost::db::Db::open(&dir.0).expect("open and migrate"));
    let conn = tenp::open_raw(&dir.0);

    let total = foreign_keys_covered(&conn).expect("every foreign key column to tenants has a leading index");
    assert!(total >= 118, "pragma_foreign_key_list lists the columns: {total}");

    let (file, sql) = tombstone_migration();
    assert!(file.ends_with("_synthetic_tenant_purge.sql"), "{file}");
    let created = created_indexes(&sql);
    let fk_indexes: Vec<&(String, String, String)> = created
        .iter()
        .filter(|(_, table, column)| {
            tenp::foreign_keys(&conn, table).iter().any(|f| f.0 == *column && f.1 == "tenants")
        })
        .collect();
    assert!(fk_indexes.len() >= 26, "the migration adds the 26 missing foreign-key indexes: {}", fk_indexes.len());

    // Dropping any one of them fails the check, naming the table and column.
    for (name, table, column) in fk_indexes {
        conn.execute_batch(&format!("DROP INDEX \"{name}\"")).unwrap();
        let err = foreign_keys_covered(&conn).expect_err("a dropped index must fail the check");
        assert!(err.contains(&format!("{table}.{column}")), "{name}: the failure names {table}.{column}: {err}");
        conn.execute_batch(&sql_for_index(&sql, name)).unwrap();
        foreign_keys_covered(&conn).expect("restored");
    }

    // The columns the purge NULLs or deletes by value are indexed too.
    for (table, column) in [
        ("runs", "caller_tenant_id"),
        ("usage_daily", "caller_tenant_id"),
        ("tenants", "scratch_of"),
        ("index_sources", "tenant_id"),
        ("catalog_receipts", "tenant_id"),
        ("invites", "org_id"),
    ] {
        let leading: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_index_list(?1) il, pragma_index_info(il.name) ii WHERE ii.seqno = 0 AND ii.name = ?2",
                params![table, column],
                |r| r.get(0),
            )
            .unwrap();
        assert!(leading >= 1, "{table}.{column} has an index to delete by");
    }
}

/// The statement of `sql` that creates index `name`.
fn sql_for_index(sql: &str, name: &str) -> String {
    let uncommented: String = sql.lines().filter(|l| !l.trim_start().starts_with("--")).collect::<Vec<_>>().join("\n");
    uncommented
        .split(';')
        .map(str::trim)
        .find(|s| s.starts_with(&format!("CREATE INDEX IF NOT EXISTS {name} ")))
        .unwrap_or_else(|| panic!("no statement creates {name}"))
        .to_string()
}

#[test]
fn tenp_ac03_migration_creates_tombstones_holding_each_tenant_delete_namespace() {
    let dir = TempDataDir::new();
    drop(mcphost::db::Db::open(&dir.0).expect("open and migrate"));
    let conn = tenp::open_raw(&dir.0);

    let columns: Vec<(String, String, bool)> = conn
        .prepare("SELECT name, type, \"notnull\" FROM pragma_table_info('namespace_tombstones') ORDER BY cid")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(
        columns,
        vec![
            ("namespace".to_string(), "TEXT".to_string(), false),
            ("purged_unix".to_string(), "INTEGER".to_string(), true),
            ("action".to_string(), "TEXT".to_string(), true)
        ]
    );
    let primary_key: i64 = conn
        .query_row("SELECT pk FROM pragma_table_info('namespace_tombstones') WHERE name = 'namespace'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(primary_key, 1, "namespace is the primary key");

    // A fresh database has no admin_events row when the migration runs, so the
    // backfill is re-run, verbatim from the migration file, over planted rows.
    let planted = [
        ("t_aaaa0001", "tenant_delete"),
        ("t_aaaa0002", "tenant_purge_expired"),
        ("t_aaaa0003", "tenant_delete"),
    ];
    let ignored = [("t_bbbb0001", "tenant_enable"), ("t_bbbb0002", "implicit_signup_rolled_back")];
    for (ns, action) in planted.iter().chain(ignored.iter()) {
        conn.execute(
            "INSERT INTO admin_events (ts, action, tenant, detail) VALUES ('2026-10-01T12:00:00Z', ?1, ?2, '{}')",
            params![action, ns],
        )
        .unwrap();
    }
    // The same namespace deleted twice keeps one tombstone.
    conn.execute(
        "INSERT INTO admin_events (ts, action, tenant, detail) VALUES ('2026-10-02T12:00:00Z', 'tenant_delete', 't_aaaa0001', '{}')",
        [],
    )
    .unwrap();
    let (_, sql) = tombstone_migration();
    let backfill = backfill_statement(&sql);
    for _ in 0..2 {
        conn.execute_batch(&backfill).expect("the backfill is idempotent");
    }
    let rows: Vec<(String, String)> = conn
        .prepare("SELECT namespace, action FROM namespace_tombstones ORDER BY namespace")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    let expected: Vec<(String, String)> = planted.iter().map(|(n, a)| (n.to_string(), a.to_string())).collect();
    assert_eq!(rows, expected, "one tombstone per planted tenant-delete namespace, with its action");
    let purged: i64 = conn
        .query_row("SELECT purged_unix FROM namespace_tombstones WHERE namespace = 't_aaaa0001'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(purged, 1_790_856_000, "the first delete dates the tombstone (2026-10-01T12:00:00Z)");
    let names: BTreeSet<String> = rows.into_iter().map(|r| r.0).collect();
    assert!(!names.contains("t_bbbb0001") && !names.contains("t_bbbb0002"), "only delete actions are tombstoned");
}
