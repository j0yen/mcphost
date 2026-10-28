//! PRD-mcphost-data-retention: per-table retention windows, the nightly
//! prune, and the disk-floor write guard.
//!
//! `db.rs` owns the `retention_policy`/`prune_log` tables and
//! `Db::prune_once` (the DB-facing half of this feature, including the
//! dedicated second `rusqlite::Connection` the actual batched deletes run
//! on -- see that method's doc comment for why); this module owns the
//! policy registry (which tables are prunable, by what column, and their
//! env-configurable defaults), the batched-delete SQL each table needs,
//! the disk-space check, and the nightly scheduler task.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::errors::AppError;
use rusqlite::{Connection, params};

/// Requirement 2: delete in batches of 5,000 so one prune cycle never
/// holds a single DELETE's row-lock scope for the whole table at once.
pub const PRUNE_BATCH_SIZE: i64 = 5_000;

/// Requirement 4 default: refuse writes when free space drops below this
/// many megabytes.
pub const DEFAULT_DISK_FLOOR_MB: u64 = 512;

/// One entry per env-configurable retention window (requirement 1). A
/// table not listed here is never pruned (AC3).
///
/// `calls_resource_usage` is intentionally listed with no matching
/// physical table to prune: those columns (`cpu_ms`, `peak_rss_kb`) live
/// directly on `calls` (migration 0004 added them as extra nullable
/// columns, not a separate table), so pruning `calls` by the `calls`
/// window already removes them. Its own window is still tracked here,
/// separately configurable, purely for `host.usage`/`admin.usage`
/// observability -- see [`EXECUTABLE_TABLES`], which has no entry for it.
pub struct PolicyTable {
    pub name: &'static str,
    pub env_var: &'static str,
    pub default_days: i64,
}

pub const POLICY_TABLES: &[PolicyTable] = &[
    PolicyTable {
        name: "calls",
        env_var: "MCPHOST_RETENTION_CALLS_DAYS",
        default_days: 90,
    },
    PolicyTable {
        name: "calls_resource_usage",
        env_var: "MCPHOST_RETENTION_CALLS_RESOURCE_USAGE_DAYS",
        default_days: 90,
    },
    PolicyTable {
        name: "events",
        env_var: "MCPHOST_RETENTION_EVENTS_DAYS",
        default_days: 30,
    },
    PolicyTable {
        name: "signup_events",
        env_var: "MCPHOST_RETENTION_SIGNUP_EVENTS_DAYS",
        default_days: 400,
    },
    PolicyTable {
        name: "metering",
        env_var: "MCPHOST_RETENTION_METERING_DAYS",
        default_days: 400,
    },
    PolicyTable {
        name: "runs",
        env_var: "MCPHOST_RETENTION_RUNS_DAYS",
        default_days: 30,
    },
    PolicyTable {
        name: "threads",
        env_var: "MCPHOST_RETENTION_THREADS_DAYS",
        default_days: 90,
    },
    PolicyTable {
        name: "messages",
        env_var: "MCPHOST_RETENTION_MESSAGES_DAYS",
        default_days: 90,
    },
    // PRD-mcphost-docs-telemetry-failed-after-search R5.
    PolicyTable {
        name: "help_search_events",
        env_var: "MCPHOST_RETENTION_HELP_SEARCH_EVENTS_DAYS",
        default_days: 30,
    },
    // PRD-mcphost-blob-retention-and-quota R3: not an age window like the
    // rows above but a grace period -- a blob is removed once its own
    // `expires_at` plus this many days has passed. Executed by
    // [`prune_blobs`] (rows and files), not by [`EXECUTABLE_TABLES`].
    PolicyTable {
        name: "blobs",
        env_var: "MCPHOST_RETENTION_BLOBS_GRACE_DAYS",
        default_days: 1,
    },
    // PRD-mcphost-synthetic-implicit-tenant-expiry: an idle window, not a
    // row-DELETE; executed by `Db::prune_once` through
    // `control::expire_idle_synthetic_tenants`. `0` switches it off.
    PolicyTable {
        name: SYNTHETIC_IMPLICIT_POLICY,
        env_var: SYNTHETIC_IMPLICIT_ENV,
        default_days: SYNTHETIC_IMPLICIT_DEFAULT_DAYS,
    },
    // PRD-mcphost-external-unclaimed-tenant-expiry: the idle window of the
    // external unclaimed policy (warn at window - lead, invalidate at window,
    // purge at window + grace); executed by `control::sweep_external_unclaimed`.
    PolicyTable {
        name: EXTERNAL_UNCLAIMED_POLICY,
        env_var: EXTERNAL_UNCLAIMED_ENV,
        default_days: EXTERNAL_UNCLAIMED_DEFAULT_DAYS,
    },
];

/// PRD-mcphost-external-unclaimed-tenant-expiry: the `retention_policy` row,
/// window env var and default (Joe 2026-10-22: key invalid at idle day 30).
pub const EXTERNAL_UNCLAIMED_POLICY: &str = "external_unclaimed_tenants";
pub const EXTERNAL_UNCLAIMED_ENV: &str = "MCPHOST_RETENTION_EXTERNAL_UNCLAIMED_TENANT_IDLE_DAYS";
pub const EXTERNAL_UNCLAIMED_DEFAULT_DAYS: i64 = 30;
/// Days between the warning and the invalidation (warn at idle day 20).
pub const WARN_LEAD_ENV: &str = "MCPHOST_EXTERNAL_UNCLAIMED_WARN_LEAD_DAYS";
pub const WARN_LEAD_DEFAULT_DAYS: i64 = 10;
/// Days between the invalidation and the purge (purge at idle day 44).
pub const PURGE_GRACE_ENV: &str = "MCPHOST_EXTERNAL_UNCLAIMED_PURGE_GRACE_DAYS";
pub const PURGE_GRACE_DEFAULT_DAYS: i64 = 14;
/// Most warnings (and notices) one sweep sends.
pub const WARN_BATCH_SIZE: i64 = 500;
/// How many pre-floor rows the first sweep lists by name.
pub const PRE_FLOOR_DRY_RUN_LIMIT: usize = 200;
/// The stage names `tenant_lifecycle_events.stage` holds.
pub const LIFECYCLE_STAGES: &[&str] =
    &["warned", "warn_no_address", "warn_failed", "invalidated", "notice_failed", "restored", "purged"];
/// `admin_events.action` of a purge.
pub const PURGE_ACTION: &str = "tenant_purge_expired";
/// What `host.doctor` and `admin.tenants` call the external policy.
pub const EXTERNAL_UNCLAIMED_LABEL: &str = "external_unclaimed";

/// The shared off-switch parse: the literal `off` (any case, trimmed).
pub fn is_off_switch(raw: &str) -> bool {
    raw.trim().eq_ignore_ascii_case("off")
}

/// `true` iff `var` is set to the literal `off`.
pub fn env_is_off(var: &str) -> bool {
    std::env::var(var).is_ok_and(|raw| is_off_switch(&raw))
}

/// The three windows of the external policy, read once per sweep.
#[derive(Debug, Clone, Copy)]
pub struct ExternalWindows {
    pub window: i64,
    pub lead: i64,
    pub grace: i64,
}

impl ExternalWindows {
    /// The window from the policy row, lead and grace from their env vars.
    pub fn read(window: i64) -> Self {
        Self { window, lead: warn_lead_days(), grace: purge_grace_days() }
    }

    /// `(warn_at, invalidate_at, purge_at)` in days from the idle run's start.
    pub fn stage_days(&self) -> (i64, i64, i64) {
        external_stage_days(self.window, self.lead, self.grace)
    }
}

/// Days between warning and invalidation.
pub fn warn_lead_days() -> i64 {
    days_from_env(WARN_LEAD_ENV, WARN_LEAD_DEFAULT_DAYS)
}

/// Days between invalidation and purge.
pub fn purge_grace_days() -> i64 {
    days_from_env(PURGE_GRACE_ENV, PURGE_GRACE_DEFAULT_DAYS)
}

/// The three stage days of a window: `(warn_at, invalidate_at, purge_at)`,
/// each counted from the start of the idle run.
pub fn external_stage_days(window: i64, lead: i64, grace: i64) -> (i64, i64, i64) {
    ((window - lead).max(0), window, window + grace)
}

/// The `retention_policy` row of the synthetic-implicit-tenant expiry.
pub const SYNTHETIC_IMPLICIT_POLICY: &str = "synthetic_implicit_tenants";
pub const SYNTHETIC_IMPLICIT_ENV: &str = "MCPHOST_RETENTION_SYNTHETIC_IMPLICIT_TENANT_IDLE_DAYS";
pub const SYNTHETIC_IMPLICIT_DEFAULT_DAYS: i64 = 1;
/// The two `tenants` values the selector names, shared by the predicate
/// (`db.rs`) and the exported contract so neither can drift.
pub const SELECTOR_SIGNUP_SOURCE: &str = "implicit";
pub const SELECTOR_ORIGIN: &str = "synthetic";

/// The default idle window the contract advertises. A `test-support` build
/// reads `MCPHOST_TEST_ONLY_TENANT_LIFECYCLE_DEFAULT_DAYS` so a test can move
/// it and watch `contract dump --check` fail; a release build never does.
pub fn contract_default_days() -> i64 {
    if cfg!(feature = "test-support")
        && let Some(days) = std::env::var("MCPHOST_TEST_ONLY_TENANT_LIFECYCLE_DEFAULT_DAYS")
            .ok()
            .and_then(|raw| raw.parse::<i64>().ok())
    {
        return days;
    }
    SYNTHETIC_IMPLICIT_DEFAULT_DAYS
}

/// `host.doctor`'s `tenant_lifecycle` for one tenant: `expires_at` is
/// [`crate::db::Db::tenant_expires_at`] (`None` = not a subject). `policy`
/// names only the external unclaimed policy, so it is null here.
pub fn lifecycle_view(idle_days: i64, expires_at: Option<i64>) -> serde_json::Value {
    match expires_at {
        Some(at) => serde_json::json!({
            "subject": true, "policy": null, "idle_days": idle_days, "expires_at_unix": at
        }),
        None => serde_json::json!({"subject": false, "policy": null, "expires_at_unix": null}),
    }
}

/// The lifecycle of an `EXT` tenant as `host.doctor` and `admin.tenants`
/// show it. `now` is the caller's clock; the three stage stamps are
/// `idle_since` plus the warn, invalidate and purge days.
pub fn external_lifecycle_view(subject: &crate::db::ExternalSubject, window: i64, now: i64) -> serde_json::Value {
    let (warn_day, expire_day, purge_day) = external_stage_days(window, warn_lead_days(), purge_grace_days());
    let since = subject.idle_since_unix;
    serde_json::json!({
        "subject": true,
        "policy": EXTERNAL_UNCLAIMED_LABEL,
        "idle_since_unix": since,
        "idle_days": ((now - since) / 86_400).max(0),
        "warn_at_unix": since + warn_day * 86_400,
        "expires_at_unix": since + expire_day * 86_400,
        "purge_at_unix": since + purge_day * 86_400,
        "warned_unix": subject.warned_unix,
        "warn_address_masked": subject.address.as_deref().map(crate::claim::mask_email),
    })
}

/// Tenants claimed and torn down per batch (measured on 6,000 subjects).
pub const EXPIRE_BATCH_SIZE: i64 = 500;

// PRD-mcphost-synthetic-tenant-purge: the purge stage's constants. The
// contract (`api_contract::tenant_lifecycle`) publishes these, so a moved
// number fails `contract dump --check`.
/// The purge grace env (days after the policy floor / last event; `off` stops the stage).
pub const SYNTHETIC_PURGE_GRACE_ENV: &str = "MCPHOST_RETENTION_SYNTHETIC_PURGE_GRACE_DAYS";
pub const SYNTHETIC_PURGE_GRACE_DEFAULT_DAYS: i64 = 14;
/// Subjects purged per batch.
pub const PURGE_BATCH: i64 = 500;
/// Wall budget of the purge stage (and, per stage, of the expire stage).
pub const PURGE_BUDGET_SECS: u64 = 120;
pub const EXPIRE_BUDGET_SECS: u64 = PURGE_BUDGET_SECS;
/// Pages each purge batch (and the sweep) returns to the OS (8 MiB).
pub const PURGE_VACUUM_PAGES: i64 = 2048;
/// Most `pre_floor_sample` rows the first armed sweep lists.
pub const PRE_FLOOR_SAMPLE_LIMIT: usize = 200;
/// `admin_events.action` of a synthetic purge.
pub const SYNTHETIC_PURGE_ACTION: &str = "tenant_purge_synthetic";
/// The `disabled_reason`s the purge class accepts.
pub const PURGE_DISABLED_REASONS: &[&str] = &["idle_expired", "self_offboard"];
/// Most random namespace draws before a mint gives up with `table_row_conflict`.
pub const NAMESPACE_DRAW_LIMIT: usize = 16;

/// The default purge grace the contract advertises. A `test-support` build reads
/// `MCPHOST_TEST_ONLY_TENANT_LIFECYCLE_PURGE_GRACE_DEFAULT_DAYS` so a test can move it and watch
/// `contract dump --check` fail; a release build never does.
pub fn contract_purge_grace_days() -> i64 {
    if cfg!(feature = "test-support")
        && let Some(days) = std::env::var("MCPHOST_TEST_ONLY_TENANT_LIFECYCLE_PURGE_GRACE_DEFAULT_DAYS")
            .ok()
            .and_then(|raw| raw.parse::<i64>().ok())
    {
        return days;
    }
    SYNTHETIC_PURGE_GRACE_DEFAULT_DAYS
}

/// `PRAGMA incremental_vacuum(pages)`: returns up to `pages` freelist pages to the file. The pragma
/// frees one page per step, so every row is drained (`execute_batch` would stop after the first).
pub(crate) fn incremental_vacuum(conn: &rusqlite::Connection, pages: i64) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA incremental_vacuum({pages})"))?;
    let mut rows = stmt.query([])?;
    while rows.next()?.is_some() {}
    Ok(())
}

/// The stage wall budget. A `test-support` build reads
/// `MCPHOST_TEST_ONLY_TENANT_LIFECYCLE_BUDGET_SECS` (seconds, fractions allowed)
/// so a test can make one sweep unable to finish; a release build never does.
pub fn stage_budget() -> std::time::Duration {
    if cfg!(feature = "test-support")
        && let Some(secs) = std::env::var("MCPHOST_TEST_ONLY_TENANT_LIFECYCLE_BUDGET_SECS")
            .ok()
            .and_then(|raw| raw.parse::<f64>().ok())
            .filter(|s| s.is_finite() && *s >= 0.0)
    {
        return std::time::Duration::from_secs_f64(secs);
    }
    std::time::Duration::from_secs(PURGE_BUDGET_SECS)
}

/// The purge grace in days; `None` when the env reads `off`.
pub fn synthetic_purge_grace_days() -> Option<i64> {
    if env_is_off(SYNTHETIC_PURGE_GRACE_ENV) {
        return None;
    }
    Some(days_from_env(SYNTHETIC_PURGE_GRACE_ENV, SYNTHETIC_PURGE_GRACE_DEFAULT_DAYS))
}

/// A policy table's window as the seed stores it. Every row ignores `0`
/// (falls back to its default) except the synthetic tenant-expiry row, where
/// `0` is an off switch; both tenant rows also accept the literal `off`.
pub fn policy_days(t: &PolicyTable) -> i64 {
    if env_is_off(t.env_var) && (t.name == SYNTHETIC_IMPLICIT_POLICY || t.name == EXTERNAL_UNCLAIMED_POLICY) {
        return 0;
    }
    if t.name == SYNTHETIC_IMPLICIT_POLICY {
        return std::env::var(t.env_var)
            .ok()
            .and_then(|raw| raw.parse::<i64>().ok())
            .filter(|d| *d >= 0)
            .unwrap_or(t.default_days);
    }
    days_from_env(t.env_var, t.default_days)
}

/// Reads one policy table's window from its env var, falling back to
/// `default_days` when unset, empty, non-numeric, or non-positive.
pub fn days_from_env(var: &str, default_days: i64) -> i64 {
    std::env::var(var)
        .ok()
        .and_then(|raw| raw.parse::<i64>().ok())
        .filter(|d| *d > 0)
        .unwrap_or(default_days)
}

/// The age column a prunable table's rows are compared against, and how
/// to render a unix-seconds cutoff for it.
enum AgeColumn {
    /// An `INTEGER` column storing unix seconds (e.g. `calls.started_unix`).
    UnixSeconds(&'static str),
    /// An `INTEGER` column storing unix milliseconds (e.g.
    /// `messages.created_unix_ms`).
    UnixMillis(&'static str),
    /// A `TEXT` column storing an RFC 3339 UTC timestamp -- sorts
    /// lexicographically in chronological order, same comparison approach
    /// `Db::emitted_call_count_for_tenant` already uses for `meter_events`.
    Rfc3339Text(&'static str),
}

impl AgeColumn {
    fn column_name(&self) -> &'static str {
        match self {
            AgeColumn::UnixSeconds(c) | AgeColumn::UnixMillis(c) | AgeColumn::Rfc3339Text(c) => c,
        }
    }
}

/// One physical table this prune cycle actually deletes from, keyed to a
/// [`PolicyTable::name`] above for its configured window.
struct ExecutableTable {
    policy_name: &'static str,
    physical_table: &'static str,
    age_column: AgeColumn,
    /// An additional `AND`-ed condition, e.g. `runs`' "only finished rows"
    /// restriction (requirement 1: "runs (finished) 30 d").
    extra_where: Option<&'static str>,
}

const EXECUTABLE_TABLES: &[ExecutableTable] = &[
    ExecutableTable {
        policy_name: "calls",
        physical_table: "calls",
        age_column: AgeColumn::UnixSeconds("started_unix"),
        extra_where: None,
    },
    ExecutableTable {
        policy_name: "events",
        physical_table: "event_dedupe",
        age_column: AgeColumn::UnixSeconds("created_unix"),
        extra_where: None,
    },
    ExecutableTable {
        policy_name: "signup_events",
        physical_table: "signup_events",
        age_column: AgeColumn::UnixSeconds("created_unix"),
        extra_where: None,
    },
    ExecutableTable {
        policy_name: "metering",
        physical_table: "meter_events",
        age_column: AgeColumn::Rfc3339Text("created_at"),
        extra_where: None,
    },
    ExecutableTable {
        policy_name: "runs",
        physical_table: "runs",
        age_column: AgeColumn::UnixSeconds("finished_unix"),
        extra_where: Some(
            "status IN ('done', 'error', 'timeout', 'cancelled') AND finished_unix IS NOT NULL",
        ),
    },
    ExecutableTable {
        policy_name: "threads",
        physical_table: "threads",
        age_column: AgeColumn::UnixMillis("created_unix_ms"),
        extra_where: None,
    },
    ExecutableTable {
        policy_name: "help_search_events",
        physical_table: "help_search_events",
        age_column: AgeColumn::UnixSeconds("searched_unix"),
        extra_where: None,
    },
    ExecutableTable {
        policy_name: "messages",
        physical_table: "messages",
        age_column: AgeColumn::UnixMillis("created_unix_ms"),
        extra_where: None,
    },
];

/// R5: the one-line sweep summary logged after every prune cycle, e.g.
/// `retention sweep: calls=0 help_search_events=12 ...`.
pub fn sweep_log_line(deleted: &std::collections::BTreeMap<String, i64>) -> String {
    let parts: Vec<String> = deleted.iter().map(|(table, n)| format!("{table}={n}")).collect();
    format!("retention sweep: {}", parts.join(" "))
}

/// A cutoff value ready to bind into a `WHERE <age_column> < ?1` clause,
/// already rendered in whichever unit/format that column stores.
enum Cutoff {
    Int(i64),
    Text(String),
}

fn cutoff_for(column: &AgeColumn, window_days: i64, now_unix: i64) -> Cutoff {
    let cutoff_unix = now_unix - window_days.saturating_mul(86_400);
    match column {
        AgeColumn::UnixSeconds(_) => Cutoff::Int(cutoff_unix),
        AgeColumn::UnixMillis(_) => Cutoff::Int(cutoff_unix.saturating_mul(1000)),
        AgeColumn::Rfc3339Text(_) => Cutoff::Text(crate::state::rfc3339_from_unix(cutoff_unix)),
    }
}

/// Deletes every row of `table` older than `cutoff`, in batches of
/// [`PRUNE_BATCH_SIZE`] (requirement 2) -- the `rowid IN (SELECT rowid ...
/// LIMIT ...)` shape works for any table (`INTEGER PRIMARY KEY` alias or a
/// plain implicit rowid alike) without needing `SQLITE_ENABLE_UPDATE_
/// DELETE_LIMIT`, which this crate's bundled `rusqlite` build does not
/// enable. Each batch is its own implicit (autocommit) transaction, so a
/// concurrent `host.tool_call` write on the main connection is blocked for
/// at most one batch, not the whole cycle -- the `prune` role's
/// `busy_timeout` (`db::DbConfig`, set through `db::open_with_role`)
/// covers the wait rather than failing it with `database is locked` (AC4).
fn delete_batches(
    conn: &Connection,
    table: &ExecutableTable,
    cutoff: &Cutoff,
) -> rusqlite::Result<i64> {
    let column = table.age_column.column_name();
    let where_sql = match table.extra_where {
        Some(extra) => format!("{column} < ?1 AND {extra}"),
        None => format!("{column} < ?1"),
    };
    let physical = table.physical_table;
    let sql = format!(
        "DELETE FROM {physical} WHERE rowid IN \
         (SELECT rowid FROM {physical} WHERE {where_sql} LIMIT {PRUNE_BATCH_SIZE})"
    );
    let mut total = 0i64;
    loop {
        let affected = match cutoff {
            Cutoff::Int(n) => conn.execute(&sql, params![n])?,
            Cutoff::Text(s) => conn.execute(&sql, params![s])?,
        } as i64;
        total += affected;
        if affected < PRUNE_BATCH_SIZE {
            break;
        }
    }
    Ok(total)
}

/// PRD-mcphost-call-audit-export R4: deletes `calls` rows older than `days`
/// for every tenant on `plan`, in [`PRUNE_BATCH_SIZE`] batches.
fn delete_plan_calls(conn: &Connection, plan: &str, days: i64, now_unix: i64) -> rusqlite::Result<i64> {
    let cutoff = now_unix - days.saturating_mul(86_400);
    let sql = format!(
        "DELETE FROM calls WHERE rowid IN (SELECT rowid FROM calls WHERE started_unix < ?1 \
         AND tenant_id IN (SELECT id FROM tenants WHERE plan = ?2) LIMIT {PRUNE_BATCH_SIZE})"
    );
    let mut total = 0i64;
    loop {
        let affected = conn.execute(&sql, params![cutoff, plan])? as i64;
        total += affected;
        if affected < PRUNE_BATCH_SIZE {
            break;
        }
    }
    Ok(total)
}

/// What one [`purge_expired_blobs`] pass removed.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct BlobPurge {
    pub rows: i64,
    pub bytes: i64,
}

fn unlink_quiet(path: &Path) {
    // A missing file is the normal crash-between-steps case; anything else
    // is left for the orphan sweep to retry.
    let _ = std::fs::remove_file(path);
}

/// PRD-mcphost-blob-retention-and-quota R3/R5: removes every blob row whose
/// `expires_at` is at or before `cutoff` (and every superseded version the
/// tenant policy aged out), unlinking each file right after its row goes, in
/// batches of [`PRUNE_BATCH_SIZE`]. `tenant` limits the pass to one tenant
/// (`host.blob.gc {dry_run: false}`); `None` is the nightly prune. A blob's
/// remaining superseded versions go with it.
pub fn purge_expired_blobs(
    conn: &Connection,
    data_dir: &Path,
    cutoff: i64,
    tenant: Option<i64>,
) -> Result<BlobPurge, AppError> {
    let mut out = BlobPurge::default();
    loop {
        let batch: Vec<(i64, String, i64)> = {
            let mut stmt = conn.prepare(
                "SELECT tenant_id, id, bytes FROM blobs WHERE expires_at IS NOT NULL \
                 AND expires_at <= ?1 AND (?2 IS NULL OR tenant_id = ?2) LIMIT ?3",
            )?;
            stmt.query_map(params![cutoff, tenant, PRUNE_BATCH_SIZE], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        crate::txn::with_tx(conn, crate::txn::TxBehavior::Immediate, |conn| {
            for (tid, id, bytes) in &batch {
                unlink_quiet(&crate::blobs::blob_file(data_dir, *tid, id));
                let versions: Vec<(i64, i64)> = {
                    let mut stmt = conn
                        .prepare("SELECT version, bytes FROM blob_versions WHERE tenant_id = ?1 AND id = ?2")?;
                    stmt.query_map(params![tid, id], |r| Ok((r.get(0)?, r.get(1)?)))?
                        .collect::<rusqlite::Result<_>>()?
                };
                for (v, vbytes) in versions {
                    unlink_quiet(&crate::blobs::version_file(data_dir, *tid, id, v));
                    out.rows += 1;
                    out.bytes += vbytes;
                }
                conn.execute("DELETE FROM blob_versions WHERE tenant_id = ?1 AND id = ?2", params![tid, id])?;
                conn.execute("DELETE FROM blobs WHERE tenant_id = ?1 AND id = ?2", params![tid, id])?;
                out.rows += 1;
                out.bytes += bytes;
            }
            Ok(())
        })?;
        if (batch.len() as i64) < PRUNE_BATCH_SIZE {
            break;
        }
    }
    loop {
        let batch: Vec<(i64, String, i64, i64)> = {
            let mut stmt = conn.prepare(
                "SELECT tenant_id, id, version, bytes FROM blob_versions WHERE expires_at IS NOT NULL \
                 AND expires_at <= ?1 AND (?2 IS NULL OR tenant_id = ?2) LIMIT ?3",
            )?;
            stmt.query_map(params![cutoff, tenant, PRUNE_BATCH_SIZE], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?
            .collect::<rusqlite::Result<_>>()?
        };
        crate::txn::with_tx(conn, crate::txn::TxBehavior::Immediate, |conn| {
            for (tid, id, version, bytes) in &batch {
                unlink_quiet(&crate::blobs::version_file(data_dir, *tid, id, *version));
                conn.execute(
                    "DELETE FROM blob_versions WHERE tenant_id = ?1 AND id = ?2 AND version = ?3",
                    params![tid, id, version],
                )?;
                out.rows += 1;
                out.bytes += bytes;
            }
            Ok(())
        })?;
        if (batch.len() as i64) < PRUNE_BATCH_SIZE {
            break;
        }
    }
    Ok(out)
}

/// What one [`prune_blobs`] pass did: rows (blobs and aged-out versions)
/// removed and their bytes, files under `blobs/` that no row owned, and rows
/// whose file was gone.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct BlobsPrune {
    pub rows: i64,
    pub bytes: i64,
    pub orphans: i64,
    pub missing: i64,
}

/// A file this young may belong to a put between its `rename` and its row
/// insert (or a `.part` still being written), not an orphan: the sweep
/// leaves it for the next night.
const BLOB_FILE_GRACE: Duration = Duration::from_secs(3600);

/// PRD-mcphost-blob-retention-and-quota R3: the nightly blob pass -- delete
/// rows whose `expires_at` + `grace_days` has passed and unlink their files
/// ([`purge_expired_blobs`]), then unlink files under `blobs/<tenant>/` that
/// no row owns and are over an hour old (a crash between row delete and
/// unlink leaves one), and mark
/// `missing` the rows whose file is gone.
pub(crate) fn prune_blobs(
    conn: &Connection,
    data_dir: &Path,
    now_unix: i64,
    grace_days: i64,
) -> Result<BlobsPrune, AppError> {
    use std::collections::HashSet;
    let cutoff = now_unix - grace_days.saturating_mul(86_400);
    let purged = purge_expired_blobs(conn, data_dir, cutoff, None)?;
    let mut out = BlobsPrune { rows: purged.rows, bytes: purged.bytes, ..BlobsPrune::default() };

    let root = data_dir.join("blobs");
    let mut tenants: std::collections::BTreeSet<i64> = std::collections::BTreeSet::new();
    if let Ok(entries) = std::fs::read_dir(&root) {
        tenants.extend(entries.flatten().filter_map(|e| e.file_name().to_str()?.parse::<i64>().ok()));
    }
    {
        let mut stmt = conn.prepare("SELECT DISTINCT tenant_id FROM blobs")?;
        tenants.extend(stmt.query_map([], |r| r.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?);
    }
    for tid in tenants {
        let dir = root.join(tid.to_string());
        let mut owned: HashSet<String> = HashSet::new();
        let mut current: Vec<(String, bool)> = Vec::new();
        {
            let mut stmt = conn.prepare("SELECT id, missing FROM blobs WHERE tenant_id = ?1")?;
            for row in stmt.query_map(params![tid], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? != 0)))? {
                let (id, missing) = row?;
                owned.insert(id.clone());
                current.push((id, missing));
            }
            let mut stmt = conn.prepare("SELECT id, version FROM blob_versions WHERE tenant_id = ?1")?;
            for row in stmt.query_map(params![tid], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
                let (id, version) = row?;
                owned.insert(format!("{id}.v{version}"));
            }
        }
        let mut on_disk: HashSet<String> = HashSet::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                    continue;
                };
                if owned.contains(&name) {
                    on_disk.insert(name);
                    continue;
                }
                let in_flight = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|age| age < BLOB_FILE_GRACE);
                if !in_flight && std::fs::remove_file(entry.path()).is_ok() {
                    out.orphans += 1;
                }
            }
        }
        for (id, was_missing) in current {
            if !was_missing && !on_disk.contains(&id) {
                conn.execute(
                    "UPDATE blobs SET missing = 1 WHERE tenant_id = ?1 AND id = ?2",
                    params![tid, id],
                )?;
                out.missing += 1;
            }
        }
    }
    Ok(out)
}

/// PRD-mcphost-notes-to-future-self R4: deletes the `notes/` rows of
/// `tenant_state_kv` whose JSON `expires_unix` is at or before `now_unix`
/// (every reader already ignores them before this runs); rows with no
/// `expires_unix`, and first-contact notes, are untouched.
pub(crate) fn prune_expired_notes(conn: &Connection, now_unix: i64) -> rusqlite::Result<i64> {
    let n = conn.execute(
        "DELETE FROM tenant_state_kv WHERE key LIKE 'notes/%' AND json_valid(value_json) \
         AND json_type(value_json) = 'object' \
         AND json_extract(value_json, '$.expires_unix') IS NOT NULL \
         AND json_extract(value_json, '$.expires_unix') <= ?1",
        params![now_unix],
    )?;
    Ok(n as i64)
}

/// One prune cycle's outcome: per-physical-table deleted counts, and the
/// first error encountered (if any) naming which table it happened on.
/// `deleted` keeps every table pruned before the failure, so a partial
/// cycle's real progress is still journaled (AC9's `last_prune_ok: false`
/// still carries whatever did complete).
pub struct PruneOutcome {
    pub deleted: std::collections::BTreeMap<String, i64>,
    pub error: Option<String>,
}

/// Runs one full prune cycle on its own dedicated connection to `db_path`
/// -- deliberately NOT `Db`'s shared `Arc<Mutex<Connection>>` (technical
/// considerations: "Prune must not hold the write lock longer than
/// busy_timeout per batch"). Sharing the app's own connection would
/// serialize every batch behind the same in-process mutex every
/// `host.tool_call` already contends on, whereas a second real SQLite
/// connection lets two independent write transactions actually contend at
/// the database level -- the scenario `busy_timeout` (rather than an
/// in-process lock) exists to cover, and the one AC4 exercises.
#[allow(clippy::too_many_arguments)]
pub(crate) fn prune_sync(
    db_path: &Path,
    db_cfg: &crate::db::DbConfig,
    counters: &std::sync::Arc<crate::db::DbCounters>,
    windows: &[(String, i64)],
    plan_calls_days: &[(String, i64)],
    now_unix: i64,
) -> PruneOutcome {
    crate::db::instrument_block(counters, crate::db::ROLE_PRUNE, || {
        prune_sync_inner(db_path, db_cfg, windows, plan_calls_days, now_unix)
    })
}

fn prune_sync_inner(
    db_path: &Path,
    db_cfg: &crate::db::DbConfig,
    windows: &[(String, i64)],
    plan_calls_days: &[(String, i64)],
    now_unix: i64,
) -> PruneOutcome {
    let mut deleted = std::collections::BTreeMap::new();
    // PRD-mcphost-sqlite-busy-timeout-audit requirement 1: opens through
    // the single factory (role `prune`), which sets `busy_timeout` from
    // `db_cfg` and `foreign_keys=ON` -- needed here (a fresh connection to
    // an existing file does not inherit `foreign_keys=ON` from the
    // connection that set it; SQLite: it's a per-connection pragma) so
    // deleting an old `threads` row cascades onto its
    // `messages`/`thread_participants`/`message_receipts` (migration
    // 0021's own cascade shape) instead of orphaning them.
    let conn = match crate::db::open_with_role(db_path, crate::db::ROLE_PRUNE, db_cfg) {
        Ok((conn, _audit)) => conn,
        Err(e) => {
            return PruneOutcome {
                deleted,
                error: Some(format!("open: {e}")),
            };
        }
    };
    for table in EXECUTABLE_TABLES {
        let Some(&(_, days)) = windows.iter().find(|(name, _)| name == table.policy_name) else {
            continue;
        };
        let cutoff = cutoff_for(&table.age_column, days, now_unix);
        match delete_batches(&conn, table, &cutoff) {
            Ok(n) => {
                deleted.insert(table.physical_table.to_string(), n);
            }
            Err(e) => {
                // requirement 3: `delete_batches` returns a raw
                // `rusqlite::Error` (never converted through
                // `AppError::from`, which is where this normally happens),
                // so this is the one spot that needs its own call.
                crate::db::note_rusqlite_error(&e);
                return PruneOutcome {
                    deleted,
                    error: Some(format!("{}: {e}", table.physical_table)),
                };
            }
        }
    }
    // PRD-mcphost-call-audit-export R4: the plan's own `calls` window, on top
    // of the host-wide one above (the shorter of the two wins).
    for (plan, days) in plan_calls_days {
        match delete_plan_calls(&conn, plan, *days, now_unix) {
            Ok(n) => *deleted.entry("calls".to_string()).or_insert(0) += n,
            Err(e) => {
                crate::db::note_rusqlite_error(&e);
                return PruneOutcome { deleted, error: Some(format!("calls ({plan}): {e}")) };
            }
        }
    }
    // PRD-mcphost-blob-retention-and-quota R3: blobs are rows AND files, so
    // they get their own pass; the result rides in the same `deleted` map
    // (and so `prune_log.deleted_json`) as `blobs`/`blobs_bytes`/
    // `blobs_orphans`/`blobs_missing`, beside the SQL tables' entries.
    let grace_days = windows
        .iter()
        .find(|(name, _)| name == "blobs")
        .map(|(_, days)| *days)
        .unwrap_or(1);
    let data_dir = db_path.parent().unwrap_or_else(|| Path::new("."));
    match prune_blobs(&conn, data_dir, now_unix, grace_days) {
        Ok(stats) => {
            deleted.insert("blobs".to_string(), stats.rows);
            deleted.insert("blobs_bytes".to_string(), stats.bytes);
            deleted.insert("blobs_orphans".to_string(), stats.orphans);
            deleted.insert("blobs_missing".to_string(), stats.missing);
        }
        Err(e) => {
            return PruneOutcome { deleted, error: Some(format!("blobs: {e}")) };
        }
    }
    // PRD-mcphost-notes-to-future-self R4: notes past their `expires_unix`.
    match prune_expired_notes(&conn, now_unix) {
        Ok(n) => {
            deleted.insert("notes".to_string(), n);
        }
        Err(e) => {
            crate::db::note_rusqlite_error(&e);
            return PruneOutcome { deleted, error: Some(format!("notes: {e}")) };
        }
    }
    // Requirement 2: only meaningful once migration 0026 has switched the
    // database to `auto_vacuum=INCREMENTAL` (always true past that
    // migration) -- harmless (a documented no-op) on the `NONE` mode a
    // brand-new connection would otherwise never see in this crate.
    let _ = incremental_vacuum(&conn, PURGE_VACUUM_PAGES);
    PruneOutcome {
        deleted,
        error: None,
    }
}

/// Requirement 4: the disk-floor write guard. `override_free_bytes` is a
/// test-only knob (AC6: "simulated by a test hook") -- same "flip an
/// internal knob for a test" shape as [`crate::db::Db::set_query_only`].
/// `Arc<Mutex<..>>`-wrapped, like [`crate::triggers::SchedulerStatus`],
/// so cloning `AppState` (every `with_conn`/background-task spawn already
/// does this) keeps every clone reading/writing the same knob.
#[derive(Clone)]
pub struct DiskGuard {
    floor_bytes: u64,
    override_free_bytes: Arc<Mutex<Option<u64>>>,
}

impl DiskGuard {
    pub fn from_env() -> Self {
        let floor_mb = std::env::var("MCPHOST_DISK_FLOOR_MB")
            .ok()
            .and_then(|raw| raw.parse::<u64>().ok())
            .filter(|mb| *mb > 0)
            .unwrap_or(DEFAULT_DISK_FLOOR_MB);
        Self {
            floor_bytes: floor_mb.saturating_mul(1024 * 1024),
            override_free_bytes: Arc::new(Mutex::new(None)),
        }
    }

    pub fn floor_bytes(&self) -> u64 {
        self.floor_bytes
    }

    /// Free bytes on the filesystem holding `path` -- the test override
    /// when set, else a real `statvfs(2)` probe. `u64::MAX` (never below
    /// the floor) on a probe failure: a guard that fails closed on every
    /// transient `statvfs` error would turn an unrelated hiccup into a
    /// false `service_unavailable` for every write in the process, worse
    /// than the rare silent-fill this requirement is guarding against.
    pub fn free_bytes(&self, path: &Path) -> u64 {
        if let Some(bytes) = *self.override_free_bytes.lock().unwrap_or_else(|e| e.into_inner()) {
            return bytes;
        }
        statvfs_free_bytes(path).unwrap_or(u64::MAX)
    }

    pub fn is_ok(&self, path: &Path) -> bool {
        self.free_bytes(path) >= self.floor_bytes
    }

    /// Test-only: pin `free_bytes` to a fixed value instead of the real
    /// filesystem probe (AC6). `None` restores the real probe.
    pub fn set_free_bytes_override_for_test(&self, bytes: Option<u64>) {
        *self.override_free_bytes.lock().unwrap_or_else(|e| e.into_inner()) = bytes;
    }
}

fn statvfs_free_bytes(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: `libc::statvfs` is a plain-old-data (all-integer-field) struct; an all-zero bit pattern is a valid value for it.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c_path` is a valid NUL-terminated C string and `&mut stat` a valid, uniquely-owned pointer `statvfs(2)` only writes through.
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) };
    if rc != 0 {
        return None;
    }
    Some((stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64))
}

/// P0 requirement 2: the nightly prune fires at 03:30 UTC, jittered up to
/// 10 minutes so a fleet of hosts sharing this build doesn't all hit disk
/// at once. Same "never joined, tokio::spawn loop" background-task
/// lifetime convention as [`crate::triggers::spawn_scheduler`].
pub fn spawn_prune_scheduler(state: crate::state::AppState) -> tokio::task::JoinHandle<()> {
    // PRD-mcphost-blob-retention-and-quota R6: the hourly blob-expiry notice
    // tick lives and dies with the prune scheduler, started by the same call.
    drop(crate::blobs::spawn_expiry_notice_loop(state.clone(), BLOB_NOTICE_TICK));
    spawn_prune_loop(state, None)
}

/// R6: how often the blob-expiry notice tick runs.
pub const BLOB_NOTICE_TICK: Duration = Duration::from_secs(3600);

/// Test-only (AC7): the same unattended loop [`spawn_prune_scheduler`]
/// runs at real `mcphost serve` startup, but firing every `interval`
/// instead of waiting out the real 03:30 UTC cadence. AC7's own proof is a
/// live prod read the day after ship (see the trailer) -- what a test
/// *can* pin down beforehand is that this background task, wired up with
/// no `admin.prune_now`/manual trigger involved, actually calls
/// `prune_once` and keeps `last_prune` fresh on its own. Same "swap a
/// short interval in for the real cadence so a test doesn't wait a real
/// day" shape as [`crate::triggers::tick_once`] exposing a single
/// deterministic tick.
pub fn spawn_prune_scheduler_for_test(
    state: crate::state::AppState,
    interval: Duration,
) -> tokio::task::JoinHandle<()> {
    spawn_prune_loop(state, Some(interval))
}

fn spawn_prune_loop(
    state: crate::state::AppState,
    interval_override: Option<Duration>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let sleep_for = interval_override
                .unwrap_or_else(|| duration_until_next_run(crate::state::now_unix()));
            tokio::time::sleep(sleep_for).await;
            if let Err(e) = state.db.prune_once().await {
                tracing::warn!(error = %e, "nightly retention prune failed");
            }
        }
    })
}

const PRUNE_HOUR_UTC: i64 = 3;
const PRUNE_MINUTE_UTC: i64 = 30;
const PRUNE_JITTER_SECS: u64 = 10 * 60;

fn next_prune_unix(now_unix: i64) -> i64 {
    let days = now_unix.div_euclid(86_400);
    let today_run = days * 86_400 + PRUNE_HOUR_UTC * 3600 + PRUNE_MINUTE_UTC * 60;
    if today_run > now_unix {
        today_run
    } else {
        today_run + 86_400
    }
}

fn duration_until_next_run(now_unix: i64) -> Duration {
    let base = next_prune_unix(now_unix);
    let jitter = rand::random::<u64>() % PRUNE_JITTER_SECS;
    Duration::from_secs((base - now_unix).max(0) as u64 + jitter)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cutoff_unix_seconds_is_now_minus_window() {
        let cutoff = cutoff_for(&AgeColumn::UnixSeconds("started_unix"), 90, 1_000_000);
        match cutoff {
            Cutoff::Int(n) => assert_eq!(n, 1_000_000 - 90 * 86_400),
            Cutoff::Text(_) => panic!("expected an integer cutoff"),
        }
    }

    #[test]
    fn cutoff_unix_millis_scales_up() {
        let cutoff = cutoff_for(&AgeColumn::UnixMillis("created_unix_ms"), 1, 200_000);
        match cutoff {
            Cutoff::Int(n) => assert_eq!(n, (200_000 - 86_400) * 1000),
            Cutoff::Text(_) => panic!("expected an integer cutoff"),
        }
    }

    #[test]
    fn cutoff_rfc3339_text_sorts_lexicographically_with_time() {
        let cutoff = cutoff_for(&AgeColumn::Rfc3339Text("created_at"), 1, 86_400 * 10);
        match cutoff {
            Cutoff::Text(s) => assert_eq!(s, crate::state::rfc3339_from_unix(86_400 * 9)),
            Cutoff::Int(_) => panic!("expected a text cutoff"),
        }
    }

    #[test]
    fn every_policy_table_name_is_unique() {
        let mut names: Vec<&str> = POLICY_TABLES.iter().map(|t| t.name).collect();
        names.sort_unstable();
        let mut deduped = names.clone();
        deduped.dedup();
        assert_eq!(names, deduped, "duplicate PolicyTable name");
    }

    #[test]
    fn every_executable_table_names_a_real_policy_table() {
        for exec in EXECUTABLE_TABLES {
            assert!(
                POLICY_TABLES.iter().any(|p| p.name == exec.policy_name),
                "EXECUTABLE_TABLES entry '{}' has no matching PolicyTable",
                exec.policy_name
            );
        }
    }

    #[test]
    fn days_from_env_falls_back_on_missing_or_invalid() {
        assert_eq!(days_from_env("MCPHOST_RETENTION_TEST_DOES_NOT_EXIST", 42), 42);
    }

    #[test]
    fn next_prune_targets_0330_utc() {
        // 1970-01-01T00:00:00Z -> the same day's 03:30Z.
        assert_eq!(next_prune_unix(0), 3 * 3600 + 30 * 60);
        // A moment past today's run rolls to tomorrow's.
        let after = 3 * 3600 + 30 * 60 + 1;
        assert_eq!(next_prune_unix(after), 86_400 + 3 * 3600 + 30 * 60);
    }

    #[test]
    fn disk_guard_override_pins_free_bytes() {
        let guard = DiskGuard::from_env();
        guard.set_free_bytes_override_for_test(Some(10));
        assert!(!guard.is_ok(Path::new("/")));
        guard.set_free_bytes_override_for_test(Some(u64::MAX));
        assert!(guard.is_ok(Path::new("/")));
    }
}
