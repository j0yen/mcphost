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

/// How one batch runs: the sweep's clock, the grace in seconds, and whether it
/// deletes (a sweep) or only counts (the report-only sweep and the dry run).
#[derive(Clone, Copy)]
struct PurgeRun {
    now: i64,
    grace_secs: i64,
    delete: bool,
}

/// What names one subject through one column, split by the other tenant's class.
#[derive(Debug, Clone, Default)]
pub(crate) struct ColumnRefs {
    pub outcome: &'static str,
    pub synthetic_implicit: i64,
    pub synthetic_explicit: i64,
    pub other: i64,
    /// Only for an author column the schema sets to NULL (`other` is then JSON null).
    pub set_null_rows: Option<i64>,
}

impl ColumnRefs {
    fn to_json(&self) -> Value {
        match self.set_null_rows {
            Some(rows) => json!({"outcome": self.outcome, "rows": rows, "other": null}),
            None => json!({
                "outcome": self.outcome,
                "synthetic_implicit": self.synthetic_implicit,
                "synthetic_explicit": self.synthetic_explicit,
                "other": self.other,
            }),
        }
    }
}

/// Every column that names a subject from another tenant's row, by key.
pub(crate) type Refs = std::collections::BTreeMap<String, ColumnRefs>;

/// Columns a row of a tenant outside the synthetic class names the subject by.
fn entangled_columns(refs: &Refs) -> Vec<String> {
    refs.iter().filter(|(_, c)| c.set_null_rows.is_none() && c.other > 0).map(|(k, _)| k.clone()).collect()
}

fn add_refs(total: &mut Refs, refs: &Refs) {
    for (key, c) in refs {
        let t = total.entry(key.clone()).or_insert_with(|| ColumnRefs { outcome: c.outcome, ..Default::default() });
        t.synthetic_implicit += c.synthetic_implicit;
        t.synthetic_explicit += c.synthetic_explicit;
        t.other += c.other;
        if let Some(rows) = c.set_null_rows {
            *t.set_null_rows.get_or_insert(0) += rows;
        }
    }
}

fn refs_json(refs: &Refs) -> Value {
    Value::Object(refs.iter().map(|(k, c)| (k.clone(), c.to_json())).collect())
}

/// Counts the rows of other tenants that name `id`, by column and by the other
/// tenant's class (`tenant_refs::TWO_SIDED`), and the author columns the
/// schema will set to NULL. Zero columns are left out.
pub(crate) fn count_refs(conn: &Connection, id: i64) -> Result<Refs, AppError> {
    let mut refs = Refs::new();
    for r in crate::tenant_refs::TWO_SIDED {
        let sql = format!(
            "WITH x(other_id) AS ({other}) SELECT cls, COUNT(*) FROM \
             (SELECT {class} AS cls FROM x JOIN tenants o ON o.id = x.other_id) GROUP BY cls",
            other = r.other_sql,
            class = crate::tenant_refs::OTHER_CLASS_SQL,
        );
        let mut stmt = conn.prepare_cached(&sql)?;
        let mut rows = stmt.query(named_params! {":id": id})?;
        let mut col = ColumnRefs { outcome: r.outcome.as_str(), ..Default::default() };
        let mut any = false;
        while let Some(row) = rows.next()? {
            let (class, n): (String, i64) = (row.get(0)?, row.get(1)?);
            any = true;
            match class.as_str() {
                "synthetic_implicit" => col.synthetic_implicit = n,
                "synthetic_explicit" => col.synthetic_explicit = n,
                _ => col.other = n,
            }
        }
        if any {
            refs.insert(r.key.to_string(), col);
        }
    }
    for (table, column) in crate::tenant_refs::SET_NULL {
        let n: i64 = conn
            .prepare_cached(&format!("SELECT COUNT(*) FROM \"{table}\" WHERE \"{column}\" = ?1"))?
            .query_row(params![id], |r| r.get(0))?;
        if n > 0 {
            let outcome = crate::tenant_refs::Outcome::SetNull.as_str();
            refs.insert(format!("{table}.{column}"), ColumnRefs { outcome, set_null_rows: Some(n), ..Default::default() });
        }
    }
    Ok(refs)
}

/// One batch's outcome on the blocking side.
#[derive(Debug, Default)]
pub(crate) struct PurgeBatchOut {
    pub purged: Vec<Tenant>,
    /// Subjects a row of a tenant outside the synthetic class names, by column.
    pub held: Vec<(i64, Vec<String>)>,
    /// What the batch's subjects are named by, summed.
    pub refs: Refs,
    /// Milliseconds each purged subject's transaction held the write lock.
    pub hold_ms: Vec<u64>,
    pub vacuumed_pages: i64,
    pub freelist_pages: i64,
}

/// What the class re-check and the reference count decided for one subject.
enum SubjectOutcome {
    Purged(Box<Tenant>, Refs),
    /// A row of a tenant outside the synthetic class names it: kept, reported.
    Held(Refs, Vec<String>),
    /// Counted only (the run does not delete).
    Counted(Refs, Vec<String>),
    /// No longer in the class or not yet due (claimed, enabled or touched since the candidate read).
    Skipped,
}

fn freelist_count(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("PRAGMA freelist_count", [], |r| r.get(0))
}

/// Deletes one subject: re-check the class inside the immediate transaction,
/// hold it if a row of a tenant outside the synthetic class names it, else the
/// shared row executor.
fn purge_subject(conn: &Connection, id: i64, run: PurgeRun) -> Result<SubjectOutcome, AppError> {
    let behavior = if run.delete { crate::txn::TxBehavior::Immediate } else { crate::txn::TxBehavior::Deferred };
    crate::txn::with_tx(conn, behavior, |conn| {
        let sql = format!(
            "SELECT namespace FROM tenants t WHERE t.id = :id AND {class} AND {due} <= :now",
            class = purge_class("t."),
            due = purge_due_at("t."),
        );
        let namespace: Option<String> = conn
            .query_row(&sql, named_params! {":id": id, ":now": run.now, ":grace": run.grace_secs}, |r| r.get(0))
            .optional()?;
        let Some(namespace) = namespace else {
            return Ok(SubjectOutcome::Skipped);
        };
        let Some(tenant) = Db::query_tenant_by_namespace(conn, &namespace)? else {
            return Ok(SubjectOutcome::Skipped);
        };
        let refs = count_refs(conn, tenant.id)?;
        let entangled = entangled_columns(&refs);
        if !run.delete {
            return Ok(SubjectOutcome::Counted(refs, entangled));
        }
        if !entangled.is_empty() {
            return Ok(SubjectOutcome::Held(refs, entangled));
        }
        let counts = Db::count_tenant_child_rows(conn, tenant.id)?;
        let detail = json!({"cross_refs": refs_json(&refs)});
        Db::delete_tenant_rows_detail(conn, &tenant, &counts, crate::retention::SYNTHETIC_PURGE_ACTION, Some(&detail))?;
        Ok(SubjectOutcome::Purged(Box::new(tenant), refs))
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
    run: PurgeRun,
) -> Result<PurgeBatchOut, AppError> {
    instrument_block(counters, ROLE_PRUNE, || {
        let (conn, _audit) = open_with_role(path, ROLE_PRUNE, cfg)?;
        // The reference count prepares ~30 statements per subject.
        conn.set_prepared_statement_cache_capacity(128);
        let mut out = PurgeBatchOut::default();
        for id in candidates {
            let started = std::time::Instant::now();
            let outcome = purge_subject(&conn, *id, run)?;
            let held_for = started.elapsed();
            match outcome {
                SubjectOutcome::Purged(tenant, refs) => {
                    add_refs(&mut out.refs, &refs);
                    out.purged.push(*tenant);
                    out.hold_ms.push(held_for.as_millis() as u64);
                }
                SubjectOutcome::Held(refs, columns) | SubjectOutcome::Counted(refs, columns) => {
                    add_refs(&mut out.refs, &refs);
                    if !columns.is_empty() {
                        out.held.push((*id, columns));
                    }
                }
                SubjectOutcome::Skipped => {}
            }
            if run.delete && held_for > SLOW_SUBJECT {
                std::thread::sleep(held_for.min(SLOW_SUBJECT_PAUSE_MAX));
            }
        }
        if run.delete {
            let before = freelist_count(&conn)?;
            crate::retention::incremental_vacuum(&conn, crate::retention::PURGE_VACUUM_PAGES)?;
            let after = freelist_count(&conn)?;
            out.vacuumed_pages = (before - after).max(0);
            out.freelist_pages = after;
        }
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
        armed: bool,
        mut block: serde_json::Map<String, Value>,
    ) -> Result<serde_json::Map<String, Value>, AppError> {
        if window_days <= 0 || crate::retention::env_is_off(crate::retention::SYNTHETIC_IMPLICIT_ENV) {
            block.insert("expire".into(), json!("off"));
            block.insert("purge".into(), json!("off"));
            return Ok(block);
        }
        match crate::retention::synthetic_purge_grace_days() {
            None => {
                block.insert("purge".into(), json!("off"));
            }
            Some(grace_days) => block.extend(self.purge_synthetic_stage(now, grace_days, armed).await?),
        }
        // What either stage left for the next sweep, and each stage's own share.
        let held = |key: &str| block.get(key).and_then(Value::as_i64).unwrap_or(0);
        let held_over = held("expire_held_over") + held("purge_held_over");
        block.insert("held_over".into(), json!(held_over));
        Ok(block)
    }

    /// Installs the cleanup the purge stage runs on each tenant it deleted.
    pub fn set_tenant_removed(&self, hook: TenantTeardown) {
        *self.tenant_removed.write().unwrap_or_else(|e| e.into_inner()) = Some(hook);
    }

    /// The purge stage of one sweep at `now`. `delete` is false for the
    /// report-only sweep (the first one that carries the block) and for the
    /// dry run: they count every due subject and delete nothing. Returns the
    /// policy block's purge keys.
    pub(crate) async fn purge_synthetic_stage(
        &self,
        now: i64,
        grace_days: i64,
        delete: bool,
    ) -> Result<serde_json::Map<String, Value>, AppError> {
        let (due, due_unix) = self.purge_due_counts(now, grace_days).await?;
        let run = PurgeRun { now, grace_secs: grace_days.saturating_mul(86_400), delete };
        let removed = self.tenant_removed.read().unwrap_or_else(|e| e.into_inner()).clone();
        let started = std::time::Instant::now();
        let budget = crate::retention::stage_budget();
        let mut refs = Refs::new();
        let mut entangled: std::collections::BTreeMap<String, i64> = Default::default();
        let mut entangled_subjects = 0i64;
        let (mut purged, mut vacuumed, mut freelist, mut after) = (0i64, 0i64, 0i64, 0i64);
        let mut batches: Vec<Value> = Vec::new();
        loop {
            if due == 0 {
                break;
            }
            let batch_started = std::time::Instant::now();
            let ids = self.purge_candidates(now, grace_days, after, crate::retention::PURGE_BATCH).await?;
            let Some(&last) = ids.last() else { break };
            after = last;
            let (path, cfg, counters) = (self.path.clone(), self.cfg, self.counters.clone());
            let out = tokio::task::spawn_blocking(move || purge_batch_sync(&path, &cfg, &counters, &ids, run))
                .await
                .map_err(|e| AppError::Internal(e.to_string()))??;
            for tenant in &out.purged {
                if let Some(hook) = &removed {
                    hook(tenant.id).await;
                }
            }
            add_refs(&mut refs, &out.refs);
            for (_, columns) in &out.held {
                entangled_subjects += 1;
                for column in columns {
                    *entangled.entry(column.clone()).or_default() += 1;
                }
            }
            purged += out.purged.len() as i64;
            vacuumed += out.vacuumed_pages;
            freelist = out.freelist_pages;
            if delete {
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
                    held_entangled = out.held.len(),
                    wall_ms,
                    vacuumed_pages = out.vacuumed_pages,
                    "synthetic tenant purge batch"
                );
                if started.elapsed() >= budget {
                    break;
                }
            }
        }
        let held_over = if delete && due > 0 { self.purge_remaining(now, grace_days, after).await? } else { 0 };
        let mut block = serde_json::Map::new();
        block.insert("purge_due_unix".into(), json!(due_unix));
        block.insert("would_purge".into(), json!(if delete { purged + held_over } else { due - entangled_subjects }));
        block.insert("purged".into(), json!(purged));
        block.insert("purge_held_over".into(), json!(held_over));
        block.insert("held_entangled".into(), json!(entangled));
        block.insert("cross_refs".into(), refs_json(&refs));
        block.insert("vacuumed_pages".into(), json!(vacuumed));
        block.insert("freelist_pages".into(), json!(freelist));
        block.insert("batches".into(), json!(batches));
        Ok(block)
    }
}
