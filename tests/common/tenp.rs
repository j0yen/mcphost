//! PRD-mcphost-synthetic-tenant-purge: fixture rows for the `tenp_ac*` suites.

use rusqlite::{Connection, params};

pub const POLICY: &str = "synthetic_implicit_tenants";
pub const IDLE_ENV: &str = "MCPHOST_RETENTION_SYNTHETIC_IMPLICIT_TENANT_IDLE_DAYS";
pub const GRACE_ENV: &str = "MCPHOST_RETENTION_SYNTHETIC_PURGE_GRACE_DAYS";
pub const BUDGET_ENV: &str = "MCPHOST_TEST_ONLY_TENANT_LIFECYCLE_BUDGET_SECS";
pub const DAY: i64 = 86_400;

/// Rows per subject of the five tables that carry the bulk (prod's per-subject ratio).
pub const HEAVY_TABLE_ROWS: &[(&str, i64)] = &[
    ("runs", 120),
    ("logs", 110),
    ("tenant_state_kv", 100),
    ("trigger_receipts", 37),
    ("host_tool_usage", 8),
];

/// Columns a `CHECK` pins to an enum, so the generic filler cannot invent them.
const OVERRIDES: &[(&str, &str, &str)] = &[
    ("billing_events", "mode", "'test'"),
    ("meter_events", "mode", "'sent'"),
    ("org_members", "role", "'member'"),
    ("recipe_uses", "via", "'quickstart'"),
    // An old event: a generated stamp would postpone the subject's due date.
    ("tenant_lifecycle_events", "created_unix", "1000"),
];

/// Inserts `n` disabled two-marker subjects named `<prefix><i>`, last seen at `last_seen`.
pub fn insert_subjects(conn: &Connection, prefix: &str, n: i64, created: i64, last_seen: i64, reason: &str) {
    conn.execute(
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?1) \
         INSERT INTO tenants (namespace, display_name, key_hash, created_at, disabled, disabled_reason, \
         signup_source, origin, synthetic, created_unix, last_seen_unix) \
         SELECT ?2 || i, ?2 || i, 'hash-' || ?2 || i, '2026-01-01T00:00:00Z', 1, ?3, 'implicit', \
         'synthetic', 'synthorg:test', ?4, ?5 FROM n",
        params![n, prefix, reason, created, last_seen],
    )
    .expect("insert subjects");
}

/// Every table whose rows cascade from `tenants` through a foreign key.
pub fn cascaded_tables(conn: &Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare(
            "SELECT m.name FROM sqlite_master m WHERE m.type = 'table' AND m.name NOT LIKE 'sqlite_%' \
             AND m.name <> 'tenants' AND EXISTS (SELECT 1 FROM pragma_foreign_key_list(m.name) f \
             WHERE f.\"table\" = 'tenants' AND f.on_delete = 'CASCADE') ORDER BY m.name",
        )
        .expect("prepare");
    stmt.query_map([], |r| r.get::<_, String>(0)).expect("query").collect::<Result<_, _>>().expect("rows")
}

/// `rows` rows of `table` per tenant whose namespace is `LIKE ns_like`, every
/// tenant column set to the tenant and every other required column filled
/// with a unique dummy (the raw connection leaves `foreign_keys` off).
/// Returns the rows inserted.
pub fn fill_table(conn: &Connection, table: &str, rows: i64, ns_like: &str) -> usize {
    let tenant_cols: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT \"from\" FROM pragma_foreign_key_list(?1) WHERE \"table\" = 'tenants' AND on_delete = 'CASCADE'")
            .expect("prepare fks");
        stmt.query_map([table], |r| r.get(0)).expect("fks").collect::<Result<_, _>>().expect("fk rows")
    };
    let cols: Vec<(String, String, bool, bool, i64)> = {
        let mut stmt = conn
            .prepare("SELECT name, type, \"notnull\", dflt_value IS NOT NULL, pk FROM pragma_table_info(?1)")
            .expect("prepare cols");
        stmt.query_map([table], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0, r.get::<_, i64>(3)? != 0, r.get(4)?)))
            .expect("cols")
            .collect::<Result<_, _>>()
            .expect("col rows")
    };
    let pk_count = cols.iter().filter(|c| c.4 > 0).count();
    let (mut names, mut vals) = (Vec::new(), Vec::new());
    for (name, ty, not_null, has_default, pk) in &cols {
        if let Some((_, _, expr)) = OVERRIDES.iter().find(|o| o.0 == table && o.1 == name) {
            names.push(name.clone());
            vals.push((*expr).to_string());
        } else if tenant_cols.contains(name) {
            names.push(name.clone());
            vals.push("s.id".to_string());
        } else if *pk > 0 && ty.eq_ignore_ascii_case("INTEGER") && pk_count == 1 {
            continue;
        } else if (!not_null && !has_default) || (*has_default && *pk == 0) {
            continue;
        } else {
            let ty = ty.to_ascii_uppercase();
            let expr = if ty.contains("INT") {
                "s.id * 1000000 + n.i"
            } else if ty == "REAL" || ty == "FLOAT" {
                "0"
            } else if ty.contains("BLOB") {
                "X'00'"
            } else {
                "'x' || s.id || '-' || n.i"
            };
            names.push(name.clone());
            vals.push(expr.to_string());
        }
    }
    let sql = format!(
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {rows}) \
         INSERT OR IGNORE INTO \"{table}\" ({}) SELECT {} FROM tenants s, n WHERE s.namespace LIKE ?1",
        names.join(", "),
        vals.join(", "),
    );
    conn.execute(&sql, [ns_like]).unwrap_or_else(|e| panic!("fill {table}: {e}"))
}

/// One row in every cascaded table for the matching tenants, then the bulk
/// rows of the five heavy tables. Returns the tables that took no row.
pub fn fill_children(conn: &Connection, ns_like: &str, heavy_rows: &[(&str, i64)]) -> Vec<String> {
    let mut empty = Vec::new();
    for table in cascaded_tables(conn) {
        let rows = heavy_rows.iter().find(|h| h.0 == table).map_or(1, |h| h.1);
        if fill_table(conn, &table, rows, ns_like) == 0 {
            empty.push(table);
        }
    }
    empty
}

/// Rows of the five heavy tables owned by tenants whose namespace is `LIKE ns_like`.
pub fn heavy_row_count(conn: &Connection, ns_like: &str) -> i64 {
    HEAVY_TABLE_ROWS
        .iter()
        .map(|(table, _)| {
            conn.query_row(
                &format!("SELECT COUNT(*) FROM \"{table}\" WHERE tenant_id IN (SELECT id FROM tenants WHERE namespace LIKE ?1)"),
                [ns_like],
                |r| r.get::<_, i64>(0),
            )
            .expect("count")
        })
        .sum()
}

/// Moves the stored floor (test-only: the real one is the first seed time).
pub fn set_floor(conn: &Connection, unix: i64) {
    conn.execute("UPDATE retention_policy SET floor_unix = ?1 WHERE table_name = ?2", params![unix, POLICY])
        .expect("set floor");
}

/// Seeds a `prune_log` row shaped like one written before this PRD: the
/// policy's flat counts and an external block that already carries its own
/// `purge_due_unix`, but no `purge_due_unix` in the policy's block.
pub fn seed_pre_prd_prune_log(conn: &Connection, at: i64) {
    let json = serde_json::json!({
        "synthetic_implicit_tenants.expired": 0,
        "synthetic_implicit_tenants.would_expire": 0,
        "synthetic_implicit_tenants": {"expired": 0},
        "external_unclaimed_tenants": {"pre_floor": {"purge_due_unix": at}},
    });
    conn.execute(
        "INSERT INTO prune_log (started_unix, finished_unix, ok, error, deleted_json) VALUES (?1, ?1, 1, NULL, ?2)",
        params![at, json.to_string()],
    )
    .expect("seed prune_log");
}

/// The policy block of the newest `prune_log` row.
pub fn last_block(conn: &Connection) -> serde_json::Value {
    let json: String = conn
        .query_row("SELECT deleted_json FROM prune_log ORDER BY id DESC LIMIT 1", [], |r| r.get(0))
        .expect("a prune_log row");
    serde_json::from_str::<serde_json::Value>(&json).expect("json")[POLICY].clone()
}

pub fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).expect("count")
}
