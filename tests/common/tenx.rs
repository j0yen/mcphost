//! PRD-mcphost-synthetic-implicit-tenant-expiry: fixture rows for the `tenx_ac*` suites.

use rusqlite::{Connection, params};

pub const POLICY: &str = "synthetic_implicit_tenants";
pub const ENV: &str = "MCPHOST_RETENTION_SYNTHETIC_IMPLICIT_TENANT_IDLE_DAYS";
pub const DAY: i64 = 86_400;

/// One `tenants` row; `Fx::subject` is a row the sweep selects once it has idled.
#[derive(Clone)]
pub struct Fx {
    pub ns: String,
    pub signup_source: Option<&'static str>,
    pub origin: &'static str,
    pub synthetic: Option<&'static str>,
    pub claimed_unix: Option<i64>,
    pub paid_unix: Option<i64>,
    pub billing_ref: Option<&'static str>,
    pub created_unix: i64,
    pub last_seen_unix: Option<i64>,
}

impl Fx {
    /// Both synthetic markers, implicit, unclaimed, unpaid, minted `age_days`
    /// ago and last seen `idle_secs` ago (`now` is the caller's clock).
    pub fn subject(ns: &str, now: i64, age_days: i64, idle_secs: i64) -> Self {
        Self {
            ns: ns.to_string(),
            signup_source: Some("implicit"),
            origin: "synthetic",
            synthetic: Some("synthorg:test"),
            claimed_unix: None,
            paid_unix: None,
            billing_ref: None,
            created_unix: now - age_days * DAY,
            last_seen_unix: Some(now - idle_secs),
        }
    }
}

pub fn insert(conn: &Connection, fx: &Fx) -> i64 {
    conn.execute(
        "INSERT INTO tenants (namespace, display_name, key_hash, created_at, disabled, signup_source, \
         origin, synthetic, claimed_unix, paid_unix, billing_ref, created_unix, last_seen_unix) \
         VALUES (?1, ?1, ?2, '2026-01-01T00:00:00Z', 0, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            fx.ns,
            format!("hash-{}", fx.ns),
            fx.signup_source,
            fx.origin,
            fx.synthetic,
            fx.claimed_unix,
            fx.paid_unix,
            fx.billing_ref,
            fx.created_unix,
            fx.last_seen_unix
        ],
    )
    .expect("insert tenant fixture");
    conn.last_insert_rowid()
}

/// `(disabled, disabled_reason)` of one tenant row.
pub fn state(conn: &Connection, id: i64) -> (bool, Option<String>) {
    conn.query_row("SELECT disabled, disabled_reason FROM tenants WHERE id = ?1", params![id], |r| {
        Ok((r.get::<_, i64>(0)? != 0, r.get(1)?))
    })
    .expect("tenant row")
}

pub fn floor(conn: &Connection) -> Option<i64> {
    conn.query_row("SELECT floor_unix FROM retention_policy WHERE table_name = ?1", params![POLICY], |r| r.get(0))
        .expect("policy row")
}

/// Moves the stored floor (test-only: the real one is the first seed time).
pub fn set_floor(conn: &Connection, unix: i64) {
    conn.execute("UPDATE retention_policy SET floor_unix = ?1 WHERE table_name = ?2", params![unix, POLICY])
        .expect("set floor");
}

pub fn stored_days(conn: &Connection) -> i64 {
    conn.query_row("SELECT days FROM retention_policy WHERE table_name = ?1", params![POLICY], |r| r.get(0))
        .expect("policy row")
}
