//! PRD-mcphost-synthetic-tenant-purge: the purge stage of the synthetic
//! tenant policy. A two-marker, disabled, unclaimed, unverified, unpaid,
//! unbilled tenant whose last activity, last lifecycle event and the policy
//! floor are all older than the grace is deleted through the same row
//! executor the external policy uses, in batches under a wall budget, each
//! subject in its own immediate transaction on the prune role's connection.
//! The first sweep that carries the block only reports.

use super::*;

/// The purge class on table alias `p` (`"t."` or empty): what a tenant IS.
/// `disabled_reason` is the only state term; the perf fixtures
/// (`perf-<plan>`, the fixed-name tenants) are never subjects.
pub(crate) fn purge_class(p: &str) -> String {
    let reasons = crate::retention::PURGE_DISABLED_REASONS
        .iter()
        .map(|r| format!("'{r}'"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{p}origin = '{origin}' AND {p}synthetic IS NOT NULL AND {p}disabled = 1 \
         AND {p}disabled_reason IN ({reasons}) AND {p}claimed_unix IS NULL \
         AND {p}owner_verified_at IS NULL AND {p}paid_unix IS NULL AND {p}billing_ref IS NULL \
         AND {p}namespace NOT LIKE 'perf-%'",
        origin = crate::retention::SELECTOR_ORIGIN,
    )
}

/// When a purge-class row falls due: the latest of its last activity, its
/// last lifecycle event and the policy floor, plus `:grace` seconds.
pub(crate) fn purge_due_at(p: &str) -> String {
    format!(
        "(MAX(COALESCE({p}last_seen_unix, {p}created_unix), \
         COALESCE((SELECT MAX(e.created_unix) FROM tenant_lifecycle_events e WHERE e.tenant_id = {p}id), 0), \
         {EXTERNAL_FLOOR_SQL}) + :grace)"
    )
}

/// When a purge runs: the sweep's clock and the grace in seconds.
#[derive(Clone, Copy)]
struct PurgeClock {
    now: i64,
    grace_secs: i64,
}

/// One batch's outcome on the blocking side.
#[derive(Debug, Default)]
pub(crate) struct PurgeBatchOut {
    pub purged: Vec<Tenant>,
    /// Milliseconds each purged subject's transaction held the write lock.
    pub hold_ms: Vec<u64>,
    pub vacuumed_pages: i64,
    pub freelist_pages: i64,
}

/// A subject the class re-check refused to delete.
enum SubjectOutcome {
    Purged(Box<Tenant>),
    /// No longer in the class or not yet due (claimed, enabled or touched since the candidate read).
    Skipped,
}

fn freelist_count(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("PRAGMA freelist_count", [], |r| r.get(0))
}

/// Deletes one subject: re-check the class inside the immediate transaction,
/// then the shared row executor.
fn purge_subject(conn: &Connection, id: i64, clock: PurgeClock) -> Result<SubjectOutcome, AppError> {
    crate::txn::with_tx(conn, crate::txn::TxBehavior::Immediate, |conn| {
        let sql = format!(
            "SELECT namespace FROM tenants t WHERE t.id = :id AND {class} AND {due} <= :now",
            class = purge_class("t."),
            due = purge_due_at("t."),
        );
        let namespace: Option<String> = conn
            .query_row(&sql, named_params! {":id": id, ":now": clock.now, ":grace": clock.grace_secs}, |r| r.get(0))
            .optional()?;
        let Some(namespace) = namespace else {
            return Ok(SubjectOutcome::Skipped);
        };
        let Some(tenant) = Db::query_tenant_by_namespace(conn, &namespace)? else {
            return Ok(SubjectOutcome::Skipped);
        };
        let counts = Db::count_tenant_child_rows(conn, tenant.id)?;
        Db::delete_tenant_rows(conn, &tenant, &counts, crate::retention::SYNTHETIC_PURGE_ACTION)?;
        Ok(SubjectOutcome::Purged(Box::new(tenant)))
    })
}

/// One batch on a dedicated connection (the prune role's, as `prune_sync`):
/// a subject per transaction, a pause after a slow one so a writer queued
/// behind it gets the lock, and a bounded `incremental_vacuum` at the end.
fn purge_batch_sync(
    path: &Path,
    cfg: &DbConfig,
    counters: &Arc<DbCounters>,
    candidates: &[i64],
    clock: PurgeClock,
) -> Result<PurgeBatchOut, AppError> {
    instrument_block(counters, ROLE_PRUNE, || {
        let (conn, _audit) = open_with_role(path, ROLE_PRUNE, cfg)?;
        let mut out = PurgeBatchOut::default();
        for id in candidates {
            let started = std::time::Instant::now();
            let outcome = purge_subject(&conn, *id, clock)?;
            let held = started.elapsed();
            if let SubjectOutcome::Purged(tenant) = outcome {
                out.purged.push(*tenant);
                out.hold_ms.push(held.as_millis() as u64);
            }
            if held > SLOW_SUBJECT {
                std::thread::sleep(held.min(SLOW_SUBJECT_PAUSE_MAX));
            }
        }
        let before = freelist_count(&conn)?;
        conn.execute_batch(&format!("PRAGMA incremental_vacuum({});", crate::retention::PURGE_VACUUM_PAGES))?;
        let after = freelist_count(&conn)?;
        out.vacuumed_pages = (before - after).max(0);
        out.freelist_pages = after;
        Ok(out)
    })
}

/// A subject transaction longer than this earns a pause.
const SLOW_SUBJECT: std::time::Duration = std::time::Duration::from_millis(5);
/// Longest pause after a slow subject.
const SLOW_SUBJECT_PAUSE_MAX: std::time::Duration = std::time::Duration::from_millis(100);

impl Db {
    /// `true` once any `prune_log` row carries the synthetic policy's
    /// `purge_due_unix`: only an armed sweep that computed the purge block
    /// writes it. Keyed on the path inside the policy's own block (the
    /// external block carries a `purge_due_unix` of its own).
    pub(crate) async fn synthetic_purge_logged(&self) -> Result<bool, AppError> {
        self.with_conn(|conn| {
            Ok(conn
                .prepare(
                    "SELECT 1 FROM prune_log WHERE json_valid(deleted_json) \
                     AND json_type(deleted_json, ?1) IS NOT NULL LIMIT 1",
                )?
                .exists(params![format!("$.{}.purge_due_unix", crate::retention::SYNTHETIC_IMPLICIT_POLICY)])?)
        })
        .await
    }

    /// `(due, earliest due stamp)` of the purge class at `now`.
    pub(crate) async fn purge_due_counts(&self, now: i64, grace_days: i64) -> Result<(i64, Option<i64>), AppError> {
        self.with_conn(move |conn| {
            let sql = format!(
                "SELECT COALESCE(SUM({due} <= :now), 0), MIN({due}) FROM tenants t WHERE {class}",
                class = purge_class("t."),
                due = purge_due_at("t."),
            );
            Ok(conn.query_row(
                &sql,
                named_params! {":now": now, ":grace": grace_days.saturating_mul(86_400)},
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })
        .await
    }

    /// The next `limit` due subjects after id `after`, oldest id first.
    async fn purge_candidates(&self, now: i64, grace_days: i64, after: i64, limit: i64) -> Result<Vec<i64>, AppError> {
        self.with_conn(move |conn| {
            let sql = format!(
                "SELECT t.id FROM tenants t WHERE t.id > :after AND {class} AND {due} <= :now \
                 ORDER BY t.id LIMIT :limit",
                class = purge_class("t."),
                due = purge_due_at("t."),
            );
            let mut stmt = conn.prepare(&sql)?;
            let ids = stmt
                .query_map(
                    named_params! {
                        ":after": after, ":now": now, ":limit": limit,
                        ":grace": grace_days.saturating_mul(86_400),
                    },
                    |r| r.get::<_, i64>(0),
                )?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ids)
        })
        .await
    }

    /// Due subjects with an id after `after`: the part of the backlog this
    /// sweep did not reach.
    async fn purge_remaining(&self, now: i64, grace_days: i64, after: i64) -> Result<i64, AppError> {
        self.with_conn(move |conn| {
            let sql = format!(
                "SELECT COUNT(*) FROM tenants t WHERE t.id > :after AND {class} AND {due} <= :now",
                class = purge_class("t."),
                due = purge_due_at("t."),
            );
            Ok(conn.query_row(
                &sql,
                named_params! {":after": after, ":now": now, ":grace": grace_days.saturating_mul(86_400)},
                |r| r.get(0),
            )?)
        })
        .await
    }

    /// What a sweep that finds another one running logs and returns: its own
    /// `prune_log` row and nothing else.
    pub(crate) async fn busy_prune_report(&self) -> Result<PruneReport, AppError> {
        let started = crate::state::now_unix();
        let synthetic = json!({"busy": true});
        let deleted_json = json!({ crate::retention::SYNTHETIC_IMPLICIT_POLICY: synthetic }).to_string();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO prune_log (started_unix, finished_unix, ok, error, deleted_json) \
                 VALUES (?1, ?1, 1, NULL, ?2)",
                params![started, deleted_json],
            )?;
            Ok(())
        })
        .await?;
        Ok(PruneReport { deleted: Default::default(), started_unix: started, finished_unix: started, synthetic })
    }

    /// The policy block of one sweep: the purge keys, or the switch that
    /// stopped them. `window_days` is the stored idle window (`0` = off).
    pub(crate) async fn synthetic_policy_block(
        &self,
        now: i64,
        window_days: i64,
    ) -> Result<serde_json::Map<String, Value>, AppError> {
        let mut block = serde_json::Map::new();
        if window_days <= 0 || crate::retention::env_is_off(crate::retention::SYNTHETIC_IMPLICIT_ENV) {
            block.insert("expire".into(), json!("off"));
            block.insert("purge".into(), json!("off"));
            return Ok(block);
        }
        let Some(grace_days) = crate::retention::synthetic_purge_grace_days() else {
            block.insert("purge".into(), json!("off"));
            return Ok(block);
        };
        let armed = self.synthetic_purge_logged().await?;
        self.purge_synthetic_stage(now, grace_days, armed).await
    }

    /// Installs the cleanup the purge stage runs on each tenant it deleted.
    pub fn set_tenant_removed(&self, hook: TenantTeardown) {
        *self.tenant_removed.write().unwrap_or_else(|e| e.into_inner()) = Some(hook);
    }

    /// The purge stage of one sweep at `now`. `may_delete` is false for the
    /// report-only sweep (the first one that carries the block). Returns the
    /// policy block's purge keys.
    pub(crate) async fn purge_synthetic_stage(
        &self,
        now: i64,
        grace_days: i64,
        may_delete: bool,
    ) -> Result<serde_json::Map<String, Value>, AppError> {
        let (would_purge, due_unix) = self.purge_due_counts(now, grace_days).await?;
        let mut block = serde_json::Map::new();
        block.insert("purge_due_unix".into(), json!(due_unix));
        block.insert("would_purge".into(), json!(would_purge));
        let mut purged = 0i64;
        let mut vacuumed = 0i64;
        let mut freelist = 0i64;
        let mut batches: Vec<Value> = Vec::new();
        let mut after = 0i64;
        let mut held_over = 0i64;
        if may_delete && would_purge > 0 {
            let removed = self.tenant_removed.read().unwrap_or_else(|e| e.into_inner()).clone();
            let started = std::time::Instant::now();
            let budget = crate::retention::stage_budget();
            loop {
                let batch_started = std::time::Instant::now();
                let ids = self.purge_candidates(now, grace_days, after, crate::retention::PURGE_BATCH).await?;
                let Some(&last) = ids.last() else { break };
                after = last;
                let (path, cfg, counters) = (self.path.clone(), self.cfg, self.counters.clone());
                let clock = PurgeClock { now, grace_secs: grace_days.saturating_mul(86_400) };
                let out = tokio::task::spawn_blocking(move || purge_batch_sync(&path, &cfg, &counters, &ids, clock))
                .await
                .map_err(|e| AppError::Internal(e.to_string()))??;
                for tenant in &out.purged {
                    if let Some(hook) = &removed {
                        hook(tenant.id).await;
                    }
                }
                purged += out.purged.len() as i64;
                vacuumed += out.vacuumed_pages;
                freelist = out.freelist_pages;
                let wall_ms = batch_started.elapsed().as_millis() as i64;
                let mut holds = out.hold_ms.clone();
                holds.sort_unstable();
                let at = |q: usize| holds.get((holds.len() * q / 100).min(holds.len().saturating_sub(1))).copied();
                batches.push(json!({
                    "purged": out.purged.len(), "wall_ms": wall_ms,
                    "hold_ms_p50": at(50), "hold_ms_p95": at(95), "hold_ms_max": holds.last(),
                }));
                tracing::info!(
                    purged = out.purged.len(),
                    held_over = would_purge - purged,
                    wall_ms,
                    vacuumed_pages = out.vacuumed_pages,
                    "synthetic tenant purge batch"
                );
                if started.elapsed() >= budget {
                    break;
                }
            }
            held_over = self.purge_remaining(now, grace_days, after).await?;
        }
        block.insert("purged".into(), json!(purged));
        block.insert("held_over".into(), json!(held_over));
        block.insert("vacuumed_pages".into(), json!(vacuumed));
        block.insert("freelist_pages".into(), json!(freelist));
        block.insert("batches".into(), json!(batches));
        Ok(block)
    }
}
