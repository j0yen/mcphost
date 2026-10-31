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
    // A plan's own `calls` window deletes an old row in the sweep.
    ("calls", "started_unix", "CAST(strftime('%s', 'now') AS INTEGER)"),
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

/// What a counterpart tenant is, for the classes `src/tenant_refs.rs` splits by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Both markers, unclaimed, `signup_source = 'implicit'`: the synthetic class.
    SyntheticImplicit,
    /// Both markers, unclaimed, an explicit sign-up: still the synthetic class.
    SyntheticExplicit,
    /// Both markers and a claim: a claimed test account is `other`.
    Claimed,
    /// The label without the origin.
    LabelOnly,
    /// The origin without the label.
    OriginOnly,
    /// An external tenant.
    External,
}

/// Inserts one enabled tenant of `kind`, last seen at `now`, and returns its id.
pub fn insert_tenant(conn: &Connection, ns: &str, kind: Kind, now: i64) -> i64 {
    let (origin, label, source, claimed) = match kind {
        Kind::SyntheticImplicit => ("synthetic", Some("synthorg:test"), "implicit", None),
        Kind::SyntheticExplicit => ("synthetic", Some("synthorg:test"), "explicit", None),
        Kind::Claimed => ("synthetic", Some("synthorg:test"), "implicit", Some(now - DAY)),
        Kind::LabelOnly => ("external", Some("synthorg:test"), "implicit", None),
        Kind::OriginOnly => ("synthetic", None, "implicit", None),
        Kind::External => ("external", None, "implicit", None),
    };
    conn.execute(
        "INSERT INTO tenants (namespace, display_name, key_hash, created_at, disabled, signup_source, origin, \
         synthetic, claimed_unix, created_unix, last_seen_unix) \
         VALUES (?1, ?1, ?2, '2026-01-01T00:00:00Z', 0, ?3, ?4, ?5, ?6, ?7, ?7)",
        params![ns, format!("hash-{ns}"), source, origin, label, claimed, now],
    )
    .expect("insert counterpart");
    conn.last_insert_rowid()
}

static UNIQUE: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(1);

/// Inserts one row into `table` with the columns in `set` (name, SQL expression)
/// and a unique dummy in every other required column; returns its rowid.
pub fn plant(conn: &Connection, table: &str, set: &[(&str, String)]) -> i64 {
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
    let unique = UNIQUE.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let (mut names, mut vals) = (Vec::new(), Vec::new());
    for (name, ty, not_null, has_default, pk) in &cols {
        if let Some((_, expr)) = set.iter().find(|(n, _)| n == name) {
            names.push(name.clone());
            vals.push(expr.clone());
        } else if let Some((_, _, expr)) = OVERRIDES.iter().find(|o| o.0 == table && o.1 == name) {
            names.push(name.clone());
            vals.push((*expr).to_string());
        } else if *pk > 0 && ty.eq_ignore_ascii_case("INTEGER") && pk_count == 1 {
            continue;
        } else if (!not_null && !has_default && *pk == 0) || (*has_default && *pk == 0) {
            continue;
        } else {
            let ty = ty.to_ascii_uppercase();
            let expr = if ty.contains("INT") {
                unique.to_string()
            } else if ty == "REAL" || ty == "FLOAT" {
                "0".to_string()
            } else if ty.contains("BLOB") {
                "X'00'".to_string()
            } else {
                format!("'x{unique}'")
            };
            names.push(name.clone());
            vals.push(expr);
        }
    }
    let sql = format!("INSERT INTO \"{table}\" ({}) VALUES ({})", names.join(", "), vals.join(", "));
    conn.execute(&sql, []).unwrap_or_else(|e| panic!("plant {table}: {e}\n{sql}"));
    conn.last_insert_rowid()
}

/// Every non-FTS table's rows as strings, sorted, minus `skip`: a snapshot to compare.
pub fn dump(conn: &Connection, skip: &[&str]) -> std::collections::BTreeMap<String, Vec<String>> {
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name NOT LIKE '%\\_fts%' ESCAPE '\\' AND name NOT LIKE '%\\_content' ESCAPE '\\' AND sql NOT LIKE 'CREATE VIRTUAL%'")
        .expect("tables")
        .query_map([], |r| r.get(0))
        .expect("tables")
        .collect::<Result<_, _>>()
        .expect("table rows");
    let mut out = std::collections::BTreeMap::new();
    for table in tables.into_iter().filter(|t| !skip.contains(&t.as_str())) {
        let mut stmt = conn.prepare(&format!("SELECT * FROM \"{table}\"")).expect("select");
        let n = stmt.column_count();
        let mut rows: Vec<String> = stmt
            .query_map([], |r| {
                Ok((0..n).map(|i| format!("{:?}", r.get_ref(i).expect("cell"))).collect::<Vec<_>>().join("|"))
            })
            .expect("rows")
            .collect::<Result<_, _>>()
            .expect("row strings");
        rows.sort();
        out.insert(table, rows);
    }
    out
}

/// `(table, column)` of every integer column that holds a tenant id: a foreign
/// key to `tenants`, or named `tenant_id`, `*_tenant_id`, `created_by`, `scratch_of`.
pub fn tenant_columns(conn: &Connection) -> Vec<(String, String)> {
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name <> 'tenants'")
        .expect("tables")
        .query_map([], |r| r.get(0))
        .expect("tables")
        .collect::<Result<_, _>>()
        .expect("table rows");
    let mut out = Vec::new();
    for table in tables.iter().map(String::as_str).chain(std::iter::once("tenants")) {
        let mut stmt = conn
            .prepare(
                "SELECT c.name FROM pragma_table_info(?1) c WHERE (c.type LIKE 'INT%' AND (c.name = 'tenant_id' \
                 OR c.name LIKE '%\\_tenant\\_id' ESCAPE '\\' OR c.name IN ('created_by', 'scratch_of'))) \
                 OR c.name IN (SELECT f.\"from\" FROM pragma_foreign_key_list(?1) f WHERE f.\"table\" = 'tenants')",
            )
            .expect("prepare");
        for col in stmt.query_map([table], |r| r.get::<_, String>(0)).expect("cols") {
            out.push((table.to_string(), col.expect("col")));
        }
    }
    out
}

/// Asserts two [`dump`]s are equal, naming each table (and its changed rows) that differs.
pub fn assert_same_dump(
    before: &std::collections::BTreeMap<String, Vec<String>>,
    after: &std::collections::BTreeMap<String, Vec<String>>,
    what: &str,
) {
    let mut changed = Vec::new();
    for (table, rows) in after {
        let old = before.get(table).cloned().unwrap_or_default();
        if &old != rows {
            let gone: Vec<&String> = old.iter().filter(|r| !rows.contains(r)).take(3).collect();
            let new: Vec<&String> = rows.iter().filter(|r| !old.contains(r)).take(3).collect();
            changed.push(format!("{table}: removed {gone:?} added {new:?}"));
        }
    }
    assert!(changed.is_empty(), "{what}: {changed:#?}");
}

/// Widens every age window of the plain row-pruning tables to a century, so the
/// sweep's ordinary retention leaves a fixture row with an old stamp alone and a
/// before/after [`dump`] shows only what the tenant policy did.
pub fn widen_row_retention(conn: &Connection) {
    conn.execute(
        "UPDATE retention_policy SET days = 36500 WHERE table_name NOT IN (?1, 'external_unclaimed_tenants')",
        params![POLICY],
    )
    .expect("widen retention");
}
