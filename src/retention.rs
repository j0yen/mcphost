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
];

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
        policy_name: "messages",
        physical_table: "messages",
        age_column: AgeColumn::UnixMillis("created_unix_ms"),
        extra_where: None,
    },
];

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
pub(crate) fn prune_sync(
    db_path: &Path,
    db_cfg: &crate::db::DbConfig,
    counters: &std::sync::Arc<crate::db::DbCounters>,
    windows: &[(String, i64)],
    now_unix: i64,
) -> PruneOutcome {
    crate::db::instrument_block(counters, crate::db::ROLE_PRUNE, || {
        prune_sync_inner(db_path, db_cfg, windows, now_unix)
    })
}

fn prune_sync_inner(
    db_path: &Path,
    db_cfg: &crate::db::DbConfig,
    windows: &[(String, i64)],
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
    // Requirement 2: only meaningful once migration 0026 has switched the
    // database to `auto_vacuum=INCREMENTAL` (always true past that
    // migration) -- harmless (a documented no-op) on the `NONE` mode a
    // brand-new connection would otherwise never see in this crate.
    let _ = conn.execute_batch("PRAGMA incremental_vacuum;");
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
    spawn_prune_loop(state, None)
}

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
