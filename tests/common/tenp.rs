//! PRD-mcphost-synthetic-tenant-purge: fixtures for the `tenp_ac*` suites. A purge subject is a
//! disabled two-marker tenant; `Planter` plants one row per table that names a tenant, derived
//! from the migrated schema at test time (`pragma_foreign_key_list`), never a hand list.

use rusqlite::types::Value as Sql;
use rusqlite::{Connection, params};
use std::collections::BTreeMap;

pub const GRACE_ENV: &str = "MCPHOST_RETENTION_SYNTHETIC_PURGE_GRACE_DAYS";
pub const IDLE_ENV: &str = "MCPHOST_RETENTION_SYNTHETIC_IMPLICIT_TENANT_IDLE_DAYS";
pub const BUDGET_ENV: &str = "MCPHOST_TEST_ONLY_TENANT_LIFECYCLE_BUDGET_SECS";
pub const POLICY: &str = "synthetic_implicit_tenants";
pub const DAY: i64 = 86_400;

/// One `tenants` row. `purge_subject` is the row the purge selects once it is due.
#[derive(Clone)]
pub struct Row {
    pub ns: String,
    pub signup_source: Option<&'static str>,
    pub origin: &'static str,
    pub synthetic: Option<&'static str>,
    pub disabled: bool,
    pub disabled_reason: Option<&'static str>,
    pub claimed_unix: Option<i64>,
    pub owner_verified_at: Option<i64>,
    pub paid_unix: Option<i64>,
    pub billing_ref: Option<&'static str>,
    pub created_unix: i64,
    pub last_seen_unix: Option<i64>,
}

impl Row {
    /// Both markers, unclaimed, unverified, unpaid, unbilled, disabled `reason`, created `age_days`
    /// ago and last seen `idle_days` ago.
    pub fn purge_subject(ns: &str, now: i64, reason: &'static str, age_days: i64, idle_days: i64) -> Self {
        Self {
            ns: ns.to_string(),
            signup_source: Some("implicit"),
            origin: "synthetic",
            synthetic: Some("synthorg:test"),
            disabled: true,
            disabled_reason: Some(reason),
            claimed_unix: None,
            owner_verified_at: None,
            paid_unix: None,
            billing_ref: None,
            created_unix: now - age_days * DAY,
            last_seen_unix: Some(now - idle_days * DAY),
        }
    }

    /// An enabled implicit synthetic unclaimed row (a subject of the disable stage).
    pub fn implicit(ns: &str, now: i64, age_days: i64, idle_secs: i64) -> Self {
        Self {
            disabled: false,
            disabled_reason: None,
            created_unix: now - age_days * DAY,
            last_seen_unix: Some(now - idle_secs),
            ..Self::purge_subject(ns, now, "idle_expired", age_days, 0)
        }
    }
}

pub fn insert(conn: &Connection, r: &Row) -> i64 {
    conn.execute(
        "INSERT INTO tenants (namespace, display_name, key_hash, created_at, disabled, disabled_reason, \
         signup_source, origin, synthetic, claimed_unix, owner_verified_at, paid_unix, billing_ref, \
         created_unix, last_seen_unix) \
         VALUES (?1, ?1, ?2, '2026-01-01T00:00:00Z', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            r.ns,
            format!("hash-{}", r.ns),
            r.disabled as i64,
            r.disabled_reason,
            r.signup_source,
            r.origin,
            r.synthetic,
            r.claimed_unix,
            r.owner_verified_at,
            r.paid_unix,
            r.billing_ref,
            r.created_unix,
            r.last_seen_unix
        ],
    )
    .expect("insert tenant fixture");
    conn.last_insert_rowid()
}

pub fn exists(conn: &Connection, id: i64) -> bool {
    conn.query_row("SELECT COUNT(*) FROM tenants WHERE id = ?1", params![id], |r| r.get::<_, i64>(0)).unwrap() == 1
}

pub fn count(conn: &Connection, sql: &str, id: i64) -> i64 {
    conn.query_row(sql, params![id], |r| r.get(0)).expect("count")
}

pub fn set_floor(conn: &Connection, unix: i64) {
    conn.execute("UPDATE retention_policy SET floor_unix = ?1 WHERE table_name = ?2", params![unix, POLICY])
        .expect("set floor");
}

/// `(disabled, disabled_reason)` of one tenant row.
pub fn state(conn: &Connection, id: i64) -> (bool, Option<String>) {
    conn.query_row("SELECT disabled, disabled_reason FROM tenants WHERE id = ?1", params![id], |r| {
        Ok((r.get::<_, i64>(0)? != 0, r.get(1)?))
    })
    .expect("tenant row")
}

/// The newest `prune_log` row's `synthetic_implicit_tenants` block.
pub fn last_block(conn: &Connection) -> serde_json::Value {
    let json: String = conn
        .query_row("SELECT deleted_json FROM prune_log ORDER BY id DESC LIMIT 1", [], |r| r.get(0))
        .expect("a prune_log row");
    let v: serde_json::Value = serde_json::from_str(&json).expect("deleted_json");
    v[POLICY].clone()
}

/// Seeds the pre-PRD-shaped `prune_log` row: the policy block is there, `purge_due_unix` is not, so
/// the report-only gate is what a test exercises.
pub fn seed_pre_prd_log_row(conn: &Connection, at: i64) {
    let body = serde_json::json!({
        "synthetic_implicit_tenants.expired": 0,
        "synthetic_implicit_tenants.skipped_billing": 0,
        "synthetic_implicit_tenants.would_expire": 0,
        // The external policy's block names a `purge_due_unix` of its own: it must not arm the gate.
        "external_unclaimed_tenants": {"purged": 0, "pre_floor": {"count": 0, "purge_due_unix": 1}},
    });
    conn.execute(
        "INSERT INTO prune_log (started_unix, finished_unix, ok, error, deleted_json) VALUES (?1, ?1, 1, NULL, ?2)",
        params![at, body.to_string()],
    )
    .expect("seed prune_log");
}

/// A foreign-key column that references `tenants`.
#[derive(Clone, Debug)]
pub struct FkCol {
    pub table: String,
    pub column: String,
    pub on_delete: String,
}

pub fn tenant_fk_cols(conn: &Connection) -> Vec<FkCol> {
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let mut out = Vec::new();
    for t in tables {
        let mut stmt = conn
            .prepare("SELECT \"from\", on_delete FROM pragma_foreign_key_list(?1) WHERE \"table\" = 'tenants'")
            .unwrap();
        let rows: Vec<(String, String)> =
            stmt.query_map(params![t], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap();
        for (column, on_delete) in rows {
            out.push(FkCol { table: t.clone(), column, on_delete });
        }
    }
    out
}

/// What fills one column of a planted row.
enum Slot {
    Tenant,
    Int,
    Real,
    Blob,
    Text(String),
    Fixed(Sql),
}

struct Spec {
    table: String,
    sql: String,
    names: Vec<String>,
    slots: Vec<Slot>,
}

/// Plants rows into any table that names a tenant, shaped from `PRAGMA table_info`: columns with a
/// default or no `NOT NULL` keep it, a `CHECK (col IN (...))` takes its first literal, every other
/// required column gets a value unique to `(tenant, k)`. Foreign keys are off on the planting
/// connection, so a second tenant column dangles at `OTHER_BASE + k` unless the caller names it.
pub struct Planter {
    specs: BTreeMap<(String, String), Spec>,
    /// What a `*_unix` column holds: recent, so no retention window sweeps a planted row.
    clock: i64,
}

const BASE: i64 = 3_000_000_000;
/// The id a planted "other tenant" column holds when the caller names none: no tenant has it.
pub const OTHER_BASE: i64 = 2_000_000_000;

impl Planter {
    pub fn new(conn: &Connection, clock: i64) -> Self {
        let mut specs = BTreeMap::new();
        for fk in tenant_fk_cols(conn) {
            if fk.table == "tenants" {
                continue; // the self-references are planted by the test that needs them
            }
            let spec = Self::spec(conn, &fk.table, &fk.column);
            specs.insert((fk.table, fk.column), spec);
        }
        Self { specs, clock }
    }

    /// A spec for a table/column that holds a tenant id without a foreign key.
    pub fn add_plain(&mut self, conn: &Connection, table: &str, column: &str) {
        let spec = Self::spec(conn, table, column);
        self.specs.insert((table.to_string(), column.to_string()), spec);
    }

    fn spec(conn: &Connection, table: &str, tenant_col: &str) -> Spec {
        let create: String = conn
            .query_row("SELECT sql FROM sqlite_master WHERE name = ?1", params![table], |r| r.get(0))
            .unwrap_or_default();
        let mut checks: BTreeMap<String, Sql> = BTreeMap::new();
        let mut rest = create.as_str();
        while let Some(at) = rest.to_ascii_uppercase().find("CHECK") {
            rest = &rest[at + 5..];
            let t = rest.trim_start().trim_start_matches('(').trim_start();
            let col: String = t.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            let after = t[col.len()..].trim_start();
            if after.len() > 2 && after[..2].eq_ignore_ascii_case("IN") {
                let list = after[2..].trim_start().trim_start_matches('(').trim_start();
                let lit = if let Some(q) = list.strip_prefix('\'') {
                    q.split('\'').next().map(|s| Sql::Text(s.to_string()))
                } else {
                    let n: String = list.chars().take_while(|c| c.is_ascii_digit() || *c == '-').collect();
                    n.parse::<i64>().ok().map(Sql::Integer)
                };
                if let Some(lit) = lit {
                    checks.entry(col).or_insert(lit);
                }
            }
        }
        let infos: Vec<(String, String, bool, bool, i64)> = conn
            .prepare("SELECT name, type, \"notnull\", dflt_value IS NOT NULL, pk FROM pragma_table_info(?1)")
            .unwrap()
            .query_map(params![table], |r| {
                Ok((r.get(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)? != 0, r.get::<_, i64>(3)? != 0, r.get(4)?))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let pk_cols = infos.iter().filter(|i| i.4 > 0).count();
        let (mut names, mut slots) = (Vec::new(), Vec::new());
        for (name, ty, notnull, has_default, pk) in &infos {
            let ty = ty.to_ascii_uppercase();
            if name == tenant_col {
                names.push(name.clone());
                slots.push(Slot::Tenant);
                continue;
            }
            if *pk == 1 && pk_cols == 1 && ty == "INTEGER" {
                continue; // the rowid alias
            }
            if let Some(v) = checks.get(name) {
                names.push(name.clone());
                slots.push(Slot::Fixed(v.clone()));
                continue;
            }
            if *has_default || (!*notnull && *pk == 0) {
                continue;
            }
            names.push(name.clone());
            slots.push(if ty.contains("INT") {
                Slot::Int
            } else if ty.contains("REAL") || ty.contains("FLOA") {
                Slot::Real
            } else if ty.contains("BLOB") {
                Slot::Blob
            } else if name.ends_with("_json") {
                Slot::Fixed(Sql::Text("{}".into()))
            } else {
                Slot::Text(name.clone())
            });
        }
        let sql = if names.is_empty() {
            format!("INSERT INTO \"{table}\" DEFAULT VALUES")
        } else {
            let cols = names.iter().map(|n| format!("\"{n}\"")).collect::<Vec<_>>().join(", ");
            let marks = vec!["?"; names.len()].join(", ");
            format!("INSERT INTO \"{table}\" ({cols}) VALUES ({marks})")
        };
        Spec { table: table.to_string(), sql, names, slots }
    }

    /// Every (table, column) planted.
    pub fn keys(&self) -> Vec<(String, String)> {
        self.specs.keys().cloned().collect()
    }

    /// Plants row `k` of `table` with `column` = `tenant_id`.
    pub fn plant(&self, conn: &Connection, table: &str, column: &str, tenant_id: i64, k: i64) -> rusqlite::Result<()> {
        self.plant_with(conn, table, column, tenant_id, k, &[])
    }

    /// Every tenant-naming column a table's spec knows is planted at `tenant_id`'s side only when
    /// it is the keyed column; this is that column set for a table.
    pub fn has(&self, table: &str, column: &str) -> bool {
        self.specs.contains_key(&(table.to_string(), column.to_string()))
    }

    /// As [`Self::plant`], with `overrides` (column, value): they replace a planted column or add a
    /// nullable one, e.g. to name the other tenant of a two-tenant row or a parent row's id.
    pub fn plant_with(
        &self,
        conn: &Connection,
        table: &str,
        column: &str,
        tenant_id: i64,
        k: i64,
        overrides: &[(&str, Sql)],
    ) -> rusqlite::Result<()> {
        let spec = self.specs.get(&(table.to_string(), column.to_string())).expect("planted column");
        let base = BASE + tenant_id * 100_000 + k;
        let mut names: Vec<&str> = Vec::with_capacity(spec.names.len() + overrides.len());
        let mut values: Vec<Sql> = Vec::with_capacity(spec.names.len() + overrides.len());
        for (name, slot) in spec.names.iter().zip(&spec.slots) {
            if overrides.iter().any(|(c, _)| *c == name.as_str()) {
                continue;
            }
            names.push(name);
            values.push(match slot {
                Slot::Tenant => Sql::Integer(tenant_id),
                Slot::Int => Sql::Integer(if name.ends_with("tenant_id") || name == "created_by" {
                    OTHER_BASE + k
                } else if name.contains("expires") || name.contains("deadline") {
                    // An expiry: ahead, so no sweep treats the planted row as expired.
                    self.clock + 30 * DAY
                } else if name.contains("unix_ms") {
                    self.clock * 1000
                } else if name.contains("unix") || name.ends_with("_at") {
                    // A clock column: recent, so no retention window sweeps a planted row.
                    self.clock
                } else {
                    base
                }),
                Slot::Real => Sql::Real(1.5),
                Slot::Blob => Sql::Blob(vec![0]),
                Slot::Text(n) => Sql::Text(format!("t{tenant_id}-{}-{n}-{k}", spec.table)),
                Slot::Fixed(v) => v.clone(),
            });
        }
        for (c, v) in overrides {
            names.push(c);
            values.push(v.clone());
        }
        let sql = if overrides.is_empty() {
            spec.sql.clone()
        } else if names.is_empty() {
            format!("INSERT INTO \"{table}\" DEFAULT VALUES")
        } else {
            let cols = names.iter().map(|n| format!("\"{n}\"")).collect::<Vec<_>>().join(", ");
            format!("INSERT INTO \"{table}\" ({cols}) VALUES ({})", vec!["?"; names.len()].join(", "))
        };
        let mut stmt = conn.prepare_cached(&sql)?;
        stmt.execute(rusqlite::params_from_iter(values))?;
        Ok(())
    }

    /// Plants one row for `tenant_id` in every (table, column) this planter knows, skipping none.
    pub fn plant_all(&self, conn: &Connection, tenant_id: i64) -> Vec<(String, String)> {
        let mut planted = Vec::new();
        for (i, (table, column)) in self.keys().into_iter().enumerate() {
            self.plant(conn, &table, &column, tenant_id, i as i64)
                .unwrap_or_else(|e| panic!("plant {table}.{column}: {e}"));
            planted.push((table, column));
        }
        planted
    }
}
