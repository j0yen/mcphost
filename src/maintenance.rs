//! PRD-mcphost-db-maintenance-window: the host's own daily database and disk
//! hygiene, run through [`crate::heal`] at the quietest hour of the last
//! fourteen days of `calls`.
//!
//! `DbMaintenance` runs `PRAGMA optimize`, `wal_checkpoint(TRUNCATE)` and
//! (only when the freelist is large and `auto_vacuum` is INCREMENTAL)
//! `incremental_vacuum`. Every statement has a 30-s deadline enforced by
//! `sqlite3_interrupt`. The module never runs a full-file rewrite and never
//! removes a tenant row.

use std::time::{Duration, Instant};

use rusqlite::Connection;
use serde_json::{Value, json};

use crate::db::{Db, ROLE_PRUNE, open_with_role};
use crate::errors::AppError;

/// Each maintenance statement is interrupted after this long.
pub const STMT_DEADLINE: Duration = Duration::from_secs(30);

/// UTC hour used when `calls` is too thin to compute one (4 am PDT).
pub const DEFAULT_QUIET_HOUR: u8 = 11;

/// Minimum `calls` rows (14 days x 24 h) before the computed hour is trusted.
pub const MIN_CALLS_FOR_QUIET_HOUR: i64 = 14 * 24;

/// `incremental_vacuum` runs only when the freelist exceeds this share of pages.
const FREELIST_VACUUM_RATIO: f64 = 0.05;

fn mb(bytes: u64) -> f64 {
    ((bytes as f64) / (1024.0 * 1024.0) * 100.0).round() / 100.0
}

/// The UTC hour (0-23) with the fewest `calls` rows over the last `days`
/// days; ties go to the later hour. `now` is the clock, so a test can pin it.
/// Fewer than 14 x 24 rows in the window gives [`DEFAULT_QUIET_HOUR`].
pub async fn quiet_hour_at(db: &Db, days: i64, now: i64) -> Result<u8, AppError> {
    let (path, cfg) = (db.path().to_path_buf(), db.cfg());
    let since = now - days * 86_400;
    tokio::task::spawn_blocking(move || {
        let (conn, _) = open_with_role(&path, ROLE_PRUNE, &cfg)?;
        let mut stmt = conn.prepare(
            "SELECT (started_unix / 3600) % 24 AS h, count(*) FROM calls \
             WHERE started_unix > ?1 GROUP BY h",
        )?;
        let mut per_hour = [0i64; 24];
        let rows = stmt.query_map([since], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
        for row in rows {
            let (h, n) = row?;
            if (0..24).contains(&h) {
                per_hour[h as usize] = n;
            }
        }
        let total: i64 = per_hour.iter().sum();
        if total < MIN_CALLS_FOR_QUIET_HOUR {
            return Ok(DEFAULT_QUIET_HOUR);
        }
        let mut best = 0usize;
        for (h, n) in per_hour.iter().enumerate() {
            if *n <= per_hour[best] {
                best = h;
            }
        }
        Ok(best as u8)
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?
}

/// [`quiet_hour_at`] over the last 14 days of wall-clock time.
pub async fn quiet_hour(db: &Db, days: i64) -> Result<u8, AppError> {
    quiet_hour_at(db, days, crate::state::now_unix()).await
}

/// The `{db_mb, wal_mb, page_count, freelist_count}` snapshot, read-only.
fn snapshot(conn: &Connection, db_path: &std::path::Path) -> Result<serde_json::Map<String, Value>, AppError> {
    let size = |p: &std::path::Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    let mut wal = db_path.as_os_str().to_owned();
    wal.push("-wal");
    let page_count: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
    let freelist_count: i64 = conn.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
    let mut m = serde_json::Map::new();
    m.insert("db_mb".into(), json!(mb(size(db_path))));
    m.insert("wal_mb".into(), json!(mb(size(std::path::Path::new(&wal)))));
    m.insert("page_count".into(), json!(page_count));
    m.insert("freelist_count".into(), json!(freelist_count));
    Ok(m)
}

/// `before` for `DbMaintenance`, without changing anything.
pub async fn db_before(db: &Db) -> Result<Value, AppError> {
    let (path, cfg) = (db.path().to_path_buf(), db.cfg());
    tokio::task::spawn_blocking(move || {
        let (conn, _) = open_with_role(&path, ROLE_PRUNE, &cfg)?;
        Ok(Value::Object(snapshot(&conn, &path)?))
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?
}

/// Runs `f` on `conn` and interrupts it after `deadline`. A timeout is an
/// error naming `stmt`.
fn with_deadline<T>(
    conn: &Connection,
    stmt: &str,
    deadline: Duration,
    f: impl FnOnce(&Connection) -> rusqlite::Result<T>,
) -> Result<T, AppError> {
    let handle = conn.get_interrupt_handle();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    let watchdog = std::thread::spawn(move || {
        if done_rx.recv_timeout(deadline).is_err() {
            handle.interrupt();
        }
    });
    let started = Instant::now();
    let result = f(conn);
    let _ = done_tx.send(());
    let _ = watchdog.join();
    result.map_err(|e| {
        if started.elapsed() >= deadline {
            AppError::Storage(format!("{stmt} exceeded its {}-s deadline: {e}", deadline.as_secs()))
        } else {
            AppError::Storage(format!("{stmt} failed: {e}"))
        }
    })
}

/// `wal_checkpoint(TRUNCATE)`; when a reader keeps it busy, one PASSIVE
/// checkpoint instead. Returns the mode that completed.
fn checkpoint(conn: &Connection) -> Result<&'static str, AppError> {
    let busy: i64 = with_deadline(conn, "PRAGMA wal_checkpoint(TRUNCATE)", STMT_DEADLINE, |c| {
        c.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0))
    })?;
    if busy == 0 {
        return Ok("TRUNCATE");
    }
    with_deadline(conn, "PRAGMA wal_checkpoint(PASSIVE)", STMT_DEADLINE, |c| {
        c.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(()))
    })?;
    Ok("PASSIVE")
}

/// The `DbMaintenance` body: returns `{"before": …, "after": …}` evidence.
pub async fn run_db_maintenance(db: &Db) -> Result<Value, AppError> {
    let (path, cfg) = (db.path().to_path_buf(), db.cfg());
    tokio::task::spawn_blocking(move || {
        let (conn, _) = open_with_role(&path, ROLE_PRUNE, &cfg)?;
        let before = snapshot(&conn, &path)?;

        let opt_started = Instant::now();
        with_deadline(&conn, "PRAGMA optimize", STMT_DEADLINE, |c| c.execute_batch("PRAGMA optimize;"))?;
        let optimize_ms = (opt_started.elapsed().as_millis() as i64).max(1);

        let mut checkpoint_result = checkpoint(&conn)?;

        let page_count = before["page_count"].as_i64().unwrap_or(0);
        let freelist = before["freelist_count"].as_i64().unwrap_or(0);
        let auto_vacuum: i64 = conn.query_row("PRAGMA auto_vacuum", [], |r| r.get(0))?;
        let mut vacuum_skipped = Value::Null;
        if freelist as f64 > FREELIST_VACUUM_RATIO * page_count as f64 {
            if auto_vacuum == 2 {
                with_deadline(&conn, "PRAGMA incremental_vacuum", STMT_DEADLINE, |c| {
                    // The pragma frees one page per step; drain every row.
                    let mut stmt = c.prepare(&format!("PRAGMA incremental_vacuum({freelist})"))?;
                    let mut rows = stmt.query([])?;
                    while rows.next()?.is_some() {}
                    Ok(())
                })?;
                // The vacuum's own pages sit in the WAL; fold them in.
                checkpoint_result = checkpoint(&conn)?;
            } else {
                vacuum_skipped = json!(format!("auto_vacuum={auto_vacuum}"));
            }
        }

        let mut after = snapshot(&conn, &path)?;
        after.insert("optimize_ms".into(), json!(optimize_ms));
        after.insert("checkpoint_result".into(), json!(checkpoint_result));
        if !vacuum_skipped.is_null() {
            after.insert("vacuum_skipped".into(), vacuum_skipped);
        }
        Ok(json!({"before": before, "after": after}))
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?
}

/// One `DbMaintenance` row through the heal executor.
pub async fn db_maintenance(db: &Db, now: i64, dry_run: bool) -> Result<crate::heal::HealRow, AppError> {
    let plan = if dry_run { db_before(db).await? } else { Value::Null };
    crate::heal::execute_on(
        db,
        crate::heal::HealAction::DbMaintenance,
        json!({"reason": "maintenance window"}),
        now,
        dry_run,
        plan,
        || run_db_maintenance(db),
    )
    .await
}

/// The backup job's marker, in the data dir.
pub const BACKUP_LOCK_FILE: &str = ".backup-lock";

/// A marker younger than this blocks the window; older is stale and ignored.
pub const BACKUP_LOCK_MAX_AGE_SECS: i64 = 2 * 3600;

/// When a window is refused, the next attempt comes this long after.
pub const RETRY_AFTER_REFUSAL_SECS: i64 = 3600;

/// The `reason` a refused row carries.
pub const BACKUP_REFUSAL_REASON: &str = "backup in progress";

/// What the data dir says about a backup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupLock {
    Absent,
    Active,
    Stale,
}

/// Reads `<data_dir>/.backup-lock` (by mtime) against `now`; with no marker,
/// a running `mcphost-backup` process counts as an active backup.
pub fn backup_lock(data_dir: &std::path::Path, now: i64) -> BackupLock {
    let modified = std::fs::metadata(data_dir.join(BACKUP_LOCK_FILE)).and_then(|m| m.modified());
    match modified {
        Ok(t) => {
            let at = t.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
            if now - at < BACKUP_LOCK_MAX_AGE_SECS { BackupLock::Active } else { BackupLock::Stale }
        }
        Err(_) if backup_process_running() => BackupLock::Active,
        Err(_) => BackupLock::Absent,
    }
}

/// Fallback until the backup job writes the marker: a process whose name is
/// `mcphost-backup`.
fn backup_process_running() -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else { return false };
    entries.flatten().any(|e| {
        e.file_name().to_string_lossy().bytes().all(|b| b.is_ascii_digit())
            && std::fs::read_to_string(e.path().join("comm")).is_ok_and(|c| c.trim() == "mcphost-backup")
    })
}

/// The first `hour`:02 UTC strictly after `now`.
pub fn next_window_unix(hour: u8, now: i64) -> i64 {
    let today = now.div_euclid(86_400) * 86_400 + i64::from(hour) * 3600 + 2 * 60;
    if today > now { today } else { today + 86_400 }
}

/// Entries directly under `<data_dir>/<name>`; none when the dir is absent.
fn count_entries(dir: &std::path::Path) -> i64 {
    std::fs::read_dir(dir).map_or(0, |d| d.flatten().count() as i64)
}

/// `before` for `LogRotate`, without changing anything.
pub fn log_before(data_dir: &std::path::Path) -> Value {
    json!({
        "scratch_entries": count_entries(&data_dir.join(SCRATCH_DIR)),
        "exports_entries": count_entries(&data_dir.join(crate::export::EXPORT_DIR)),
    })
}

/// The host's scratch directory, under the data dir.
pub const SCRATCH_DIR: &str = "scratch";

/// Scratch entries older than this are removed.
pub const SCRATCH_TTL_SECS: i64 = 24 * 3600;

/// Removes direct children of `dir` whose mtime is more than `ttl_secs`
/// before `now`; returns how many went. Symlinks are unlinked, never
/// followed. A missing `dir` is zero.
fn sweep_older_than(dir: &std::path::Path, now: i64, ttl_secs: i64) -> i64 {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
        let Ok(modified) = meta.modified() else { continue };
        let at = modified.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        if now - at <= ttl_secs {
            continue;
        }
        let gone = if meta.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
        match gone {
            Ok(()) => removed += 1,
            Err(e) => tracing::warn!(path = %path.display(), error = %e, "log_rotate could not remove an entry"),
        }
    }
    removed
}

/// The `LogRotate` body: sweeps expired `scratch/` and `exports/` entries
/// only, and asks the deploy tool for a journal vacuum (it needs root).
/// Returns `{"before": …, "after": …}` evidence.
pub async fn run_log_rotate(db: &Db, now: i64) -> Result<Value, AppError> {
    let data_dir = db.data_dir().to_path_buf();
    tokio::task::spawn_blocking(move || {
        let before = log_before(&data_dir);
        let scratch_removed = sweep_older_than(&data_dir.join(SCRATCH_DIR), now, SCRATCH_TTL_SECS);
        let exports_removed =
            sweep_older_than(&data_dir.join(crate::export::EXPORT_DIR), now, crate::export::EXPORT_URL_TTL_SECS);
        let mut after = log_before(&data_dir);
        after["scratch_removed"] = json!(scratch_removed);
        after["exports_removed"] = json!(exports_removed);
        after["journal_vacuum_requested"] = json!(true);
        Ok(json!({"before": before, "after": after}))
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?
}

/// A daily pass that last started longer ago than this runs in the window.
pub const HOUSEKEEPING_STALE_SECS: i64 = 20 * 3600;

/// R7: the other daily housekeeping shares the hour. The status-feed sample
/// prune is idempotent and keeps no last-run record, so it always runs; the
/// retention pruner runs when `prune_log` shows no cycle in the last 20 h.
async fn run_housekeeping(db: &Db, now: i64) -> Result<(), AppError> {
    db.prune_status_samples_older_than(now - crate::statusfeed::SAMPLE_RETENTION_DAYS * 86_400).await?;
    let last = db.last_prune_started_unix().await?;
    if last.is_none_or(|t| now - t >= HOUSEKEEPING_STALE_SECS) {
        db.prune_once().await?;
        tracing::info!("retention prune ran inside the maintenance window");
    }
    Ok(())
}

/// What one window did.
#[derive(Debug)]
pub struct WindowReport {
    pub rows: Vec<crate::heal::HealRow>,
    /// When the next attempt is due: an hour out after a refusal, else the
    /// next quiet-hour slot.
    pub next_unix: i64,
}

/// Runs the window: `DbMaintenance` then `LogRotate`, each through the heal
/// executor. A fresh backup marker refuses both (and touches nothing); a
/// stale marker is logged and ignored.
pub async fn run_window(db: &Db, now: i64, dry_run: bool) -> Result<WindowReport, AppError> {
    let data_dir = db.data_dir().to_path_buf();
    let lock = backup_lock(&data_dir, now);
    if lock == BackupLock::Stale {
        tracing::warn!("stale backup lock ignored: {} older than {} s", BACKUP_LOCK_FILE, BACKUP_LOCK_MAX_AGE_SECS);
    }
    let refuse = lock == BackupLock::Active;
    let mut rows = Vec::with_capacity(2);

    let plan = if dry_run { db_before(db).await? } else { Value::Null };
    rows.push(
        crate::heal::execute_on(
            db,
            crate::heal::HealAction::DbMaintenance,
            json!({"reason": "maintenance window"}),
            now,
            dry_run,
            plan,
            || async {
                if refuse {
                    return Ok(json!({"refused": BACKUP_REFUSAL_REASON}));
                }
                run_db_maintenance(db).await
            },
        )
        .await?,
    );

    if !dry_run && !refuse {
        run_housekeeping(db, now).await?;
    }

    // The second row is stamped after the housekeeping, so its `at` follows
    // the pruner's own log line.
    let now = now.max(crate::state::now_unix());
    let plan = if dry_run { log_before(&data_dir) } else { Value::Null };
    rows.push(
        crate::heal::execute_on(
            db,
            crate::heal::HealAction::LogRotate,
            json!({"reason": "maintenance window"}),
            now,
            dry_run,
            plan,
            || async {
                if refuse {
                    return Ok(json!({"refused": BACKUP_REFUSAL_REASON}));
                }
                run_log_rotate(db, now).await
            },
        )
        .await?,
    );

    let next_unix = if refuse {
        now + RETRY_AFTER_REFUSAL_SECS
    } else {
        next_window_unix(quiet_hour_at(db, 14, now).await?, now)
    };
    Ok(WindowReport { rows, next_unix })
}

/// The scheduler's published view: the quiet hour and when the next window
/// is due. Process-wide, as `/status.json` has no handle on the scheduler.
static SCHEDULE: std::sync::Mutex<Option<(u8, i64)>> = std::sync::Mutex::new(None);

fn publish_schedule(quiet_hour: u8, next_unix: i64) {
    if let Ok(mut s) = SCHEDULE.lock() {
        *s = Some((quiet_hour, next_unix));
    }
}

/// R5: the `maintenance` object `/status.json` carries.
pub async fn status_block(state: &crate::state::AppState) -> Result<Value, AppError> {
    let (quiet_hour_utc, next_unix) = SCHEDULE.lock().ok().and_then(|s| *s).unwrap_or_else(|| {
        (DEFAULT_QUIET_HOUR, next_window_unix(DEFAULT_QUIET_HOUR, crate::state::now_unix()))
    });
    let last = state.db.latest_heal_row(crate::heal::HealAction::DbMaintenance).await?.map(|row| {
        // A refused row carries no sizes; report the `before` ones then.
        let sizes = if row.after.get("wal_mb").is_some() { &row.after } else { &row.before };
        json!({
            "at": row.at,
            "outcome": row.outcome.as_str(),
            "db_mb": sizes.get("db_mb").cloned().unwrap_or(Value::Null),
            "wal_mb": sizes.get("wal_mb").cloned().unwrap_or(Value::Null),
        })
    });
    let journal_vacuum_requested = state
        .db
        .latest_heal_row(crate::heal::HealAction::LogRotate)
        .await?
        .is_some_and(|row| row.after["journal_vacuum_requested"] == json!(true));
    Ok(json!({
        "quiet_hour_utc": quiet_hour_utc,
        "next_unix": next_unix,
        "last": last,
        "journal_vacuum_requested": journal_vacuum_requested,
    }))
}

/// R6: fires [`run_window`] once a day at `quiet_hour_utc:02`, recomputing
/// the hour daily; a refused window is retried an hour later.
pub fn spawn_scheduler(db: Db) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut retry_at: Option<i64> = None;
        let mut hour = DEFAULT_QUIET_HOUR;
        loop {
            let now = crate::state::now_unix();
            let next = match retry_at.take() {
                Some(t) => t,
                None => {
                    hour = quiet_hour(&db, 14).await.unwrap_or_else(|e| {
                        tracing::warn!(error = %e, "quiet hour computation failed; using the default");
                        DEFAULT_QUIET_HOUR
                    });
                    let next = next_window_unix(hour, now);
                    publish_schedule(hour, next);
                    next
                }
            };
            tokio::time::sleep(Duration::from_secs((next - crate::state::now_unix()).max(0) as u64)).await;
            match run_window(&db, crate::state::now_unix(), false).await {
                Ok(report) if report.rows.iter().any(|r| r.outcome == crate::heal::HealOutcome::Refused) => {
                    retry_at = Some(report.next_unix);
                    publish_schedule(hour, report.next_unix);
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "maintenance window failed"),
            }
        }
    })
}
