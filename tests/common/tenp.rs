//! PRD-mcphost-synthetic-tenant-purge: fixtures for the `tenp_ac*` suites --
//! purge-class tenant rows, the schema enumeration of every column that names a
//! tenant, and a generic row planter over it.

use rusqlite::{Connection, params};
use std::collections::{BTreeMap, BTreeSet};

pub const GRACE_ENV: &str = "MCPHOST_RETENTION_SYNTHETIC_PURGE_GRACE_DAYS";
pub const BUDGET_ENV: &str = "MCPHOST_TEST_ONLY_TENANT_LIFECYCLE_BUDGET_SECS";
pub const ACTION: &str = "tenant_purge_synthetic";
pub const POLICY: &str = "synthetic_implicit_tenants";
pub const DAY: i64 = 86_400;

static PLANT_SEQ: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// One `tenants` row. `Row::subject` is a member of the purge class: both
/// markers, disabled `idle_expired`, unclaimed, unverified, unpaid, unbilled.
#[derive(Clone)]
pub struct Row {
    pub ns: String,
    pub origin: &'static str,
    pub synthetic: Option<&'static str>,
    pub signup_source: Option<&'static str>,
    pub disabled: bool,
    pub reason: Option<&'static str>,
    pub claimed_unix: Option<i64>,
    pub owner_verified_at: Option<i64>,
    pub paid_unix: Option<i64>,
    pub billing_ref: Option<&'static str>,
    pub created_unix: i64,
    pub last_seen_unix: Option<i64>,
}

impl Row {
    /// A purge-class row last seen at `last_seen_unix` and created a day before it.
    pub fn subject(ns: &str, last_seen_unix: i64) -> Self {
        Self {
            ns: ns.to_string(),
            origin: "synthetic",
            synthetic: Some("synthorg:test"),
            signup_source: Some("implicit"),
            disabled: true,
            reason: Some("idle_expired"),
            claimed_unix: None,
            owner_verified_at: None,
            paid_unix: None,
            billing_ref: None,
            created_unix: last_seen_unix - DAY,
            last_seen_unix: Some(last_seen_unix),
        }
    }

    /// A claimed two-marker tenant: the `other` class by the R1 rule.
    pub fn claimed(ns: &str, last_seen_unix: i64) -> Self {
        Self { claimed_unix: Some(last_seen_unix), ..Self::subject(ns, last_seen_unix) }
    }
}

pub fn insert(conn: &Connection, row: &Row) -> i64 {
    conn.execute(
        "INSERT INTO tenants (namespace, display_name, key_hash, created_at, disabled, disabled_reason, \
         signup_source, origin, synthetic, claimed_unix, owner_verified_at, paid_unix, billing_ref, \
         created_unix, last_seen_unix) \
         VALUES (?1, ?1, ?2, '2026-01-01T00:00:00Z', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            row.ns,
            format!("hash-{}", row.ns),
            row.disabled as i64,
            row.reason,
            row.signup_source,
            row.origin,
            row.synthetic,
            row.claimed_unix,
            row.owner_verified_at,
            row.paid_unix,
            row.billing_ref,
            row.created_unix,
            row.last_seen_unix
        ],
    )
    .expect("insert tenant fixture");
    conn.last_insert_rowid()
}

/// A second connection on the server's database file. Foreign keys are off:
/// fixture rows name parents (tools, threads, grants) that need not exist for
/// the cascade from `tenants` to reach them.
pub fn open_raw(dir: &std::path::Path) -> Connection {
    let conn = Connection::open(dir.join("mcphost.db")).expect("raw db");
    conn.busy_timeout(std::time::Duration::from_secs(60)).expect("busy timeout");
    conn.execute_batch("PRAGMA foreign_keys = OFF").expect("fk off");
    conn
}

/// `(count of tenants rows, count of purge audit rows)` -- exactly-once checks.
pub fn tenant_exists(conn: &Connection, id: i64) -> bool {
    conn.query_row("SELECT COUNT(*) FROM tenants WHERE id = ?1", params![id], |r| r.get::<_, i64>(0)).unwrap() == 1
}

pub fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap_or_else(|e| panic!("{sql}: {e}"))
}

/// The policy block of the newest `prune_log` row.
pub fn latest_block(conn: &Connection) -> serde_json::Value {
    let raw: String = conn
        .query_row("SELECT deleted_json FROM prune_log ORDER BY id DESC LIMIT 1", [], |r| r.get(0))
        .expect("a prune_log row");
    let all: serde_json::Value = serde_json::from_str(&raw).expect("json");
    all[POLICY].clone()
}

/// Seeds the pre-PRD-shaped `prune_log` row: the policy name present (flat
/// counters and a nested block), no `purge_due_unix`.
pub fn seed_pre_prd_row(conn: &Connection, now: i64) {
    conn.execute(
        "INSERT INTO prune_log (started_unix, finished_unix, ok, deleted_json) VALUES (?1, ?1, 1, ?2)",
        params![
            now - 3 * DAY,
            serde_json::json!({
                "synthetic_implicit_tenants.expired": 0,
                "synthetic_implicit_tenants.skipped_billing": 0,
                "synthetic_implicit_tenants.would_expire": 0,
                POLICY: {"expired": 0}
            })
            .to_string()
        ],
    )
    .expect("seed prune_log");
}

/// Arms the R1 gate: a `prune_log` row whose policy block carries `purge_due_unix`.
pub fn arm_gate(conn: &Connection, now: i64) {
    conn.execute(
        "INSERT INTO prune_log (started_unix, finished_unix, ok, deleted_json) VALUES (?1, ?1, 1, ?2)",
        params![now - 2 * DAY, serde_json::json!({ POLICY: {"purge_due_unix": null, "purged": 0} }).to_string()],
    )
    .expect("arm gate");
}

// ---- schema enumeration ----

/// One column that names a tenant, by foreign key or by name.
#[derive(Clone, Debug)]
pub struct TenantCol {
    pub table: String,
    pub column: String,
    /// `ON DELETE` action of the foreign key to `tenants`, `None` for a name-only column.
    pub fk: Option<String>,
    pub not_null: bool,
}

/// Every base table (virtual FTS tables included, their shadow tables not).
fn tables(conn: &Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare("SELECT name, sql FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .unwrap();
    let all: Vec<(String, Option<String>)> =
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(|r| r.unwrap()).collect();
    let virtuals: Vec<&String> = all
        .iter()
        .filter(|(_, sql)| sql.as_deref().is_some_and(|s| s.contains("VIRTUAL TABLE")))
        .map(|(n, _)| n)
        .collect();
    all.iter()
        .filter(|(n, _)| {
            !virtuals.iter().any(|v| {
                ["_data", "_idx", "_content", "_docsize", "_config"].iter().any(|suffix| *n == format!("{v}{suffix}"))
            })
        })
        .map(|(n, _)| n.clone())
        .collect()
}

pub fn all_tables(conn: &Connection) -> Vec<String> {
    tables(conn)
}

struct Col {
    name: String,
    ty: String,
    not_null: bool,
    default: Option<String>,
    pk: i64,
}

fn columns(conn: &Connection, table: &str) -> Vec<Col> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info(\"{table}\")")).unwrap();
    stmt.query_map([], |r| {
        Ok(Col {
            name: r.get(1)?,
            ty: r.get::<_, String>(2)?.to_uppercase(),
            not_null: r.get::<_, i64>(3)? != 0,
            default: r.get(4)?,
            pk: r.get(5)?,
        })
    })
    .unwrap()
    .map(|r| r.unwrap())
    .collect()
}

/// `pragma_foreign_key_list` rows of `table`: `(from, parent table, parent column, on_delete)`.
pub fn foreign_keys(conn: &Connection, table: &str) -> Vec<(String, String, Option<String>, String)> {
    let mut stmt = conn.prepare(&format!("PRAGMA foreign_key_list(\"{table}\")")).unwrap();
    stmt.query_map([], |r| Ok((r.get(3)?, r.get(2)?, r.get(4)?, r.get(6)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

/// Every column naming a tenant: a foreign key to `tenants`, or by name
/// (`tenant_id`, `*_tenant_id`, `scratch_of`; an integer `created_by` only through its foreign key).
pub fn tenant_columns(conn: &Connection) -> Vec<TenantCol> {
    let mut out = Vec::new();
    for table in tables(conn) {
        let fks = foreign_keys(conn, &table);
        for c in columns(conn, &table) {
            let fk = fks.iter().find(|f| f.0 == c.name && f.1 == "tenants").map(|f| f.3.clone());
            let by_name = (c.name == "tenant_id" || c.name.ends_with("_tenant_id") || c.name == "scratch_of")
                && (c.ty.contains("INT") || c.ty.is_empty());
            if fk.is_some() || by_name {
                out.push(TenantCol { table: table.clone(), column: c.name, fk, not_null: c.not_null });
            }
        }
    }
    out
}

/// The tables with at least one tenant column, with their columns.
pub fn tenant_tables(conn: &Connection) -> BTreeMap<String, Vec<TenantCol>> {
    let mut map: BTreeMap<String, Vec<TenantCol>> = BTreeMap::new();
    for c in tenant_columns(conn) {
        map.entry(c.table.clone()).or_default().push(c);
    }
    map
}

/// Values the generic planter cannot guess (`CHECK` lists).
fn check_override(table: &str, column: &str) -> Option<&'static str> {
    Some(match (table, column) {
        ("billing_events", "mode") => "'test'",
        ("meter_events", "mode") => "'sent'",
        ("org_members", "role") => "'member'",
        ("recipe_uses", "via") => "'quickstart'",
        _ => return None,
    })
}

/// A SQL expression for one column of a planted row; `key` names the SQL
/// expression that is unique per planted row (the tenant id, or a counter).
fn value_expr(table: &str, c: &Col, key: &str) -> String {
    if let Some(v) = check_override(table, &c.name) {
        return v.to_string();
    }
    // Timestamps are "now", so the generic retention prune (which ages rows out
    // by these columns) leaves the fixture alone.
    let is_int = c.ty.contains("INT") || c.ty.contains("REAL") || c.ty.contains("NUM");
    if is_int && c.name.ends_with("_unix_ms") {
        return "CAST(strftime('%s','now') AS INTEGER) * 1000".to_string();
    }
    if is_int && (c.name.ends_with("_unix") || c.name.ends_with("_at")) {
        return "CAST(strftime('%s','now') AS INTEGER)".to_string();
    }
    if is_int {
        return key.to_string();
    }
    if c.ty.contains("BLOB") {
        return "zeroblob(1)".to_string();
    }
    if c.name == "ts" || c.name.ends_with("_at") {
        return "strftime('%Y-%m-%dT%H:%M:%SZ','now')".to_string();
    }
    format!("'v' || {key}")
}

/// Plants one row per id of `ids` in `table` (the tenant ids in `assign`), set
/// based: every column in `assign` takes its expression, every other tenant
/// column takes the id, any other `NOT NULL` column without a
/// default takes a value unique per id. Returns the number of rows inserted.
///
/// `assign` entries are `(column, SQL expression over the temp table
/// `tenp_ids`)`; the expression `id` is the subject, `tenp_ids.other` the other tenant.
pub fn plant(conn: &Connection, table: &str, assign: &[(&str, &str)]) -> usize {
    // Unique per planted row: the pair, and a counter that keeps two plants of the same pair apart.
    let key = format!("(id * 1000 + other + {})", PLANT_SEQ.fetch_add(1_000_000_000, std::sync::atomic::Ordering::SeqCst));
    let cols = columns(conn, table);
    let tenant_cols: Vec<String> = tenant_columns(conn)
        .into_iter()
        .filter(|c| c.table == table)
        .map(|c| c.column)
        .collect();
    let mut names = Vec::new();
    let mut exprs = Vec::new();
    let single_pk = cols.iter().filter(|c| c.pk > 0).count() == 1;
    for c in &cols {
        if let Some((_, expr)) = assign.iter().find(|(name, _)| *name == c.name) {
            names.push(format!("\"{}\"", c.name));
            exprs.push((*expr).to_string());
            continue;
        }
        if tenant_cols.contains(&c.name) {
            names.push(format!("\"{}\"", c.name));
            exprs.push("id".to_string());
            continue;
        }
        if c.pk > 0 && single_pk && c.ty == "INTEGER" {
            continue;
        }
        if (c.not_null || c.pk > 0) && c.default.is_none() {
            names.push(format!("\"{}\"", c.name));
            exprs.push(value_expr(table, c, &key));
        }
    }
    let sql = format!(
        "INSERT INTO \"{table}\" ({}) SELECT {} FROM tenp_ids",
        names.join(", "),
        exprs.join(", ")
    );
    conn.execute(&sql, []).unwrap_or_else(|e| panic!("plant {table}: {e}\n{sql}"))
}

/// Replaces the planting id set: `(id, other)` pairs.
pub fn set_ids(conn: &Connection, pairs: &[(i64, i64)]) {
    conn.execute_batch("CREATE TEMP TABLE IF NOT EXISTS tenp_ids (id INTEGER, other INTEGER); DELETE FROM tenp_ids;")
        .unwrap();
    let mut stmt = conn.prepare("INSERT INTO tenp_ids (id, other) VALUES (?1, ?2)").unwrap();
    for (id, other) in pairs {
        stmt.execute(params![id, other]).unwrap();
    }
}

/// Rows of `table` whose `column` equals `id`.
pub fn rows_naming(conn: &Connection, table: &str, column: &str, id: i64) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM \"{table}\" WHERE \"{column}\" = ?1"), params![id], |r| r.get(0))
        .unwrap_or_else(|e| panic!("{table}.{column}: {e}"))
}

/// The newest failed `prune_log` row's error text, for a failing test's message.
pub fn latest_error(conn: &Connection) -> String {
    conn.query_row("SELECT COALESCE(error, '') FROM prune_log WHERE ok = 0 ORDER BY id DESC LIMIT 1", [], |r| r.get(0))
        .unwrap_or_else(|_| "no failed prune_log row".to_string())
}

// ---- the mechanically derived two-sided set (R3) ----

/// What the schema says is two-sided, derived from `pragma_foreign_key_list`
/// and the tenant columns alone.
pub struct Derived {
    /// (a) tables with two tenant columns, (b) the `tenants` self-references.
    pub flat_tables: BTreeSet<String>,
    /// (c) tables with a tenant column that reach, over `ON DELETE CASCADE`
    /// edges child -> parent, a parent whose own tenant column is `CASCADE`.
    pub structural_tables: BTreeSet<String>,
    /// `"<parent.column>/<child.column>"` for every (c) member and parent.
    pub structural_keys: BTreeSet<String>,
}

pub fn derive_two_sided(conn: &Connection) -> Derived {
    let by_table = tenant_tables(conn);
    let mut flat_tables = BTreeSet::new();
    for (table, cols) in &by_table {
        let names: BTreeSet<&str> = cols.iter().map(|c| c.column.as_str()).collect();
        if names.len() >= 2 || (table == "tenants" && !names.is_empty()) {
            flat_tables.insert(table.clone());
        }
    }
    // Parents a table reaches over CASCADE edges that do not point at `tenants`.
    let cascade_parents = |start: &str| -> BTreeSet<String> {
        let mut seen = BTreeSet::new();
        let mut stack = vec![start.to_string()];
        while let Some(t) = stack.pop() {
            for (_, parent, _, on_delete) in foreign_keys(conn, &t) {
                if parent != "tenants" && on_delete == "CASCADE" && parent != start && seen.insert(parent.clone()) {
                    stack.push(parent);
                }
            }
        }
        seen
    };
    let fk_cols = |table: &str, cascade_only: bool| -> Vec<String> {
        by_table
            .get(table)
            .map(|cols| {
                cols.iter()
                    .filter(|c| match &c.fk {
                        Some(od) => !cascade_only || od == "CASCADE",
                        None => false,
                    })
                    .map(|c| c.column.clone())
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut structural_tables = BTreeSet::new();
    let mut structural_keys = BTreeSet::new();
    for table in by_table.keys() {
        // A member names a tenant by foreign key; a name-only `tenant_id` is cleared by value.
        if fk_cols(table, false).is_empty() {
            continue;
        }
        for parent in cascade_parents(table) {
            let parent_cols = fk_cols(&parent, true);
            if parent_cols.is_empty() {
                continue;
            }
            structural_tables.insert(table.clone());
            for pc in &parent_cols {
                for cc in fk_cols(table, false) {
                    structural_keys.insert(format!("{parent}.{pc}/{table}.{cc}"));
                }
            }
        }
    }
    Derived { flat_tables, structural_tables, structural_keys }
}

/// A canonical text of every row of every tenant table that names one of
/// `named` and none of `excluded` -- to compare before and after a sweep.
/// `skip` tables are derived data the sweep itself rewrites.
pub fn snapshot(conn: &Connection, named: &[i64], excluded: &[i64], skip: &[&str]) -> String {
    let list = |ids: &[i64]| ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    let mut out = String::new();
    for (table, cols) in tenant_tables(conn) {
        if skip.contains(&table.as_str()) {
            continue;
        }
        let mut names: Vec<String> = cols.iter().map(|c| format!("\"{}\"", c.column)).collect();
        if table == "tenants" {
            names.push("id".to_string());
        }
        let any = |ids: &[i64]| names.iter().map(|n| format!("{n} IN ({})", list(ids))).collect::<Vec<_>>().join(" OR ");
        let all_columns: Vec<String> =
            columns(conn, &table).iter().map(|c| format!("quote(\"{}\")", c.name)).collect();
        let sql = format!(
            "SELECT {} AS r FROM \"{table}\" WHERE ({}) AND NOT ({}) ORDER BY r",
            all_columns.join(" || '|' || "),
            any(named),
            any(excluded)
        );
        let mut stmt = conn.prepare(&sql).unwrap_or_else(|e| panic!("{table}: {e}\n{sql}"));
        let rows: Vec<String> = stmt.query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect();
        for r in rows {
            out.push_str(&format!("{table}: {r}\n"));
        }
    }
    out
}

/// The lines two snapshots do not share, `-` only before and `+` only after.
pub fn diff_lines(before: &str, after: &str) -> String {
    let (b, a): (BTreeSet<&str>, BTreeSet<&str>) = (before.lines().collect(), after.lines().collect());
    let mut out = String::new();
    for l in b.difference(&a) {
        out.push_str(&format!("- {l}\n"));
    }
    for l in a.difference(&b) {
        out.push_str(&format!("+ {l}\n"));
    }
    out
}
