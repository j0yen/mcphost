//! Synthetic tenant expire and purge stages (PRD-mcphost-synthetic-tenant-purge): the idle-clock
//! disable, the bounded purge of disabled two-marker tenants, the cross-tenant reference table
//! that decides whether a subject may be deleted, and the read-only dry run.

use super::*;
use crate::retention as ret;
use std::collections::BTreeMap;
use std::time::Instant;

/// What a purge does to the other tenant's row that names the subject.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The nullable column is set to NULL; the row stays.
    Nulled,
    /// The row is deleted by an explicit statement (no foreign key to cascade).
    Delete,
    /// The row is deleted by the schema's `ON DELETE CASCADE`.
    Cascade,
    /// The column is `ON DELETE SET NULL`: no counterpart column, never held.
    SetNull,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Nulled => "nulled",
            Outcome::Delete => "delete",
            Outcome::Cascade => "cascade",
            Outcome::SetNull => "set_null",
        }
    }
}

/// One way a row of another tenant names a purge subject. `owners` yields one row per row naming
/// subject `:id`, holding the id of the OTHER tenant (the row's owner, or for a join relation the
/// child row's tenant column); a row the subject owns itself is one-sided and never listed.
pub struct CrossRef {
    pub key: &'static str,
    pub outcome: Outcome,
    pub owners: &'static str,
}

const fn x(key: &'static str, outcome: Outcome, owners: &'static str) -> CrossRef {
    CrossRef { key, outcome, owners }
}

/// § Cascade of the PRD: every cross-tenant (two-sided) column. A table added later that names
/// two tenants, or cascades from another tenant's row, fails `tenp_ac02_*` until it is listed here.
pub const CROSS_REFS: &[CrossRef] = &[
    // Two-sided by owner column, nullable: NULLed.
    x("calls.caller_tenant_id", Outcome::Nulled, "SELECT tenant_id AS owner FROM calls WHERE caller_tenant_id = :id AND tenant_id <> :id"),
    x("runs.caller_tenant_id", Outcome::Nulled, "SELECT tenant_id AS owner FROM runs WHERE caller_tenant_id = :id AND tenant_id <> :id"),
    x("tenants.invited_by_tenant_id", Outcome::Nulled, "SELECT id AS owner FROM tenants WHERE invited_by_tenant_id = :id AND id <> :id"),
    x("tenants.scratch_of", Outcome::Nulled, "SELECT id AS owner FROM tenants WHERE scratch_of = :id AND id <> :id"),
    // `NOT NULL`, no foreign key: the rows are deleted.
    x("usage_daily.caller_tenant_id", Outcome::Delete, "SELECT tenant_id AS owner FROM usage_daily WHERE caller_tenant_id = :id AND tenant_id <> :id"),
    x("invite_joins.inviter_tenant_id", Outcome::Delete, "SELECT invitee_tenant_id AS owner FROM invite_joins WHERE inviter_tenant_id = :id AND invitee_tenant_id <> :id"),
    // `NOT NULL CASCADE` on the other tenant's row.
    x("blocks.blocked_tenant_id", Outcome::Cascade, "SELECT tenant_id AS owner FROM blocks WHERE blocked_tenant_id = :id AND tenant_id <> :id"),
    x("contacts.contact_tenant_id", Outcome::Cascade, "SELECT tenant_id AS owner FROM contacts WHERE contact_tenant_id = :id AND tenant_id <> :id"),
    x("muted.muted_tenant_id", Outcome::Cascade, "SELECT tenant_id AS owner FROM muted WHERE muted_tenant_id = :id AND tenant_id <> :id"),
    x("tool_version_watermarks.caller_tenant_id", Outcome::Cascade, "SELECT tenant_id AS owner FROM tool_version_watermarks WHERE caller_tenant_id = :id AND tenant_id <> :id"),
    x("tool_caller_limits.caller_tenant_id", Outcome::Cascade, "SELECT tenant_id AS owner FROM tool_caller_limits WHERE caller_tenant_id = :id AND tenant_id <> :id"),
    // Pair tables: either direction.
    x("contact_requests.from_tenant_id", Outcome::Cascade, "SELECT to_tenant_id AS owner FROM contact_requests WHERE from_tenant_id = :id AND to_tenant_id <> :id"),
    x("contact_requests.to_tenant_id", Outcome::Cascade, "SELECT from_tenant_id AS owner FROM contact_requests WHERE to_tenant_id = :id AND from_tenant_id <> :id"),
    x("tasks.poster_tenant_id", Outcome::Cascade, "SELECT recipient_tenant_id AS owner FROM tasks WHERE poster_tenant_id = :id AND recipient_tenant_id <> :id"),
    x("tasks.recipient_tenant_id", Outcome::Cascade, "SELECT poster_tenant_id AS owner FROM tasks WHERE recipient_tenant_id = :id AND poster_tenant_id <> :id"),
    x("invite_joins.invitee_tenant_id", Outcome::Cascade, "SELECT inviter_tenant_id AS owner FROM invite_joins WHERE invitee_tenant_id = :id AND inviter_tenant_id <> :id"),
    // Structural set: a child row of a subject-owned parent, keyed `<parent.column>/<child.column>`.
    x("groups.owner_tenant_id/group_members.member_tenant_id", Outcome::Cascade,
      "SELECT m.member_tenant_id AS owner FROM groups g JOIN group_members m ON m.group_id = g.id \
       WHERE g.owner_tenant_id = :id AND m.member_tenant_id <> :id"),
    x("groups.owner_tenant_id/channels.created_by", Outcome::Cascade,
      "SELECT c.created_by AS owner FROM groups g JOIN channels c ON c.group_id = g.id \
       WHERE g.owner_tenant_id = :id AND c.created_by IS NOT NULL AND c.created_by <> :id"),
    x("groups.owner_tenant_id/channel_posts.from_tenant_id", Outcome::Cascade,
      "SELECT p.from_tenant_id AS owner FROM groups g JOIN channels c ON c.group_id = g.id \
       JOIN channel_posts p ON p.channel_id = c.id \
       WHERE g.owner_tenant_id = :id AND p.from_tenant_id IS NOT NULL AND p.from_tenant_id <> :id"),
    x("groups.owner_tenant_id/channel_cursors.tenant_id", Outcome::Cascade,
      "SELECT k.tenant_id AS owner FROM groups g JOIN channels c ON c.group_id = g.id \
       JOIN channel_cursors k ON k.channel_id = c.id \
       WHERE g.owner_tenant_id = :id AND k.tenant_id <> :id"),
    x("orgs.owner_tenant_id/org_members.tenant_id", Outcome::Cascade,
      "SELECT m.tenant_id AS owner FROM orgs o JOIN org_members m ON m.org_id = o.id \
       WHERE o.owner_tenant_id = :id AND m.tenant_id <> :id"),
    x("orgs.owner_tenant_id/invites.inviter_tenant_id", Outcome::Cascade,
      "SELECT i.inviter_tenant_id AS owner FROM orgs o JOIN invites i ON i.org_id = o.id \
       WHERE o.owner_tenant_id = :id AND i.inviter_tenant_id <> :id"),
    x("orgs.owner_tenant_id/invite_joins.invitee_tenant_id", Outcome::Cascade,
      "SELECT j.invitee_tenant_id AS owner FROM orgs o JOIN invites i ON i.org_id = o.id \
       JOIN invite_joins j ON j.invite_id = i.id \
       WHERE o.owner_tenant_id = :id AND j.invitee_tenant_id <> :id"),
    x("invites.inviter_tenant_id/invite_joins.invitee_tenant_id", Outcome::Cascade,
      "SELECT j.invitee_tenant_id AS owner FROM invites i JOIN invite_joins j ON j.invite_id = i.id \
       WHERE i.inviter_tenant_id = :id AND j.invitee_tenant_id <> :id"),
    // `ON DELETE SET NULL` author columns: counted, never held.
    x("channel_posts.from_tenant_id", Outcome::SetNull, "SELECT NULL FROM channel_posts WHERE from_tenant_id = :id"),
    x("channels.created_by", Outcome::SetNull, "SELECT NULL FROM channels WHERE created_by = :id"),
    x("messages.from_tenant_id", Outcome::SetNull, "SELECT NULL FROM messages WHERE from_tenant_id = :id"),
    x("threads.created_by", Outcome::SetNull, "SELECT NULL FROM threads WHERE created_by = :id"),
    x("signup_events.tenant_id", Outcome::SetNull, "SELECT NULL FROM signup_events WHERE tenant_id = :id"),
];

/// `synthetic_implicit`, `synthetic_explicit` or `other` for the owner `o`: decided by the owner's
/// own non-disabled terms, never by `origin` alone, so a claimed two-marker test account is `other`.
const OWNER_CLASS: &str = "CASE WHEN o.id IS NULL THEN 'other' \
    WHEN o.origin = 'synthetic' AND o.synthetic IS NOT NULL AND o.claimed_unix IS NULL \
         AND o.owner_verified_at IS NULL AND o.paid_unix IS NULL AND o.billing_ref IS NULL \
    THEN CASE WHEN o.signup_source = 'implicit' THEN 'synthetic_implicit' ELSE 'synthetic_explicit' END \
    ELSE 'other' END";

/// Rows naming one subject (or, summed, a sweep's subjects) through one column.
#[derive(Debug, Clone, Copy)]
pub struct RefCount {
    pub outcome: Outcome,
    pub synthetic_implicit: i64,
    pub synthetic_explicit: i64,
    pub other: i64,
    pub rows: i64,
}

impl RefCount {
    fn new(outcome: Outcome) -> Self {
        Self { outcome, synthetic_implicit: 0, synthetic_explicit: 0, other: 0, rows: 0 }
    }

    fn add(&mut self, rhs: &RefCount) {
        self.synthetic_implicit += rhs.synthetic_implicit;
        self.synthetic_explicit += rhs.synthetic_explicit;
        self.other += rhs.other;
        self.rows += rhs.rows;
    }

    /// `other` is `null` for a `SET NULL` column: it has no counterpart column to classify.
    pub fn to_json(self) -> Value {
        if self.outcome == Outcome::SetNull {
            return json!({
                "outcome": self.outcome.as_str(), "rows": self.rows,
                "synthetic_implicit": null, "synthetic_explicit": null, "other": null,
            });
        }
        json!({
            "outcome": self.outcome.as_str(), "rows": self.rows,
            "synthetic_implicit": self.synthetic_implicit,
            "synthetic_explicit": self.synthetic_explicit,
            "other": self.other,
        })
    }
}

/// Cross-reference counts by column key; a column with no row naming the subject is absent.
pub type Refs = BTreeMap<&'static str, RefCount>;

/// Counts, per column of [`CROSS_REFS`], the rows of other tenants that name `id`.
pub fn count_cross_refs(conn: &Connection, id: i64) -> Result<Refs, AppError> {
    let mut refs = Refs::new();
    for r in CROSS_REFS {
        let mut count = RefCount::new(r.outcome);
        if r.outcome == Outcome::SetNull {
            count.rows = conn
                .prepare_cached(&format!("SELECT COUNT(*) FROM ({})", r.owners))?
                .query_row(named_params! {":id": id}, |row| row.get(0))?;
        } else {
            let sql = format!(
                "SELECT {OWNER_CLASS}, COUNT(*) FROM ({}) x LEFT JOIN tenants o ON o.id = x.owner GROUP BY 1",
                r.owners,
            );
            let mut stmt = conn.prepare_cached(&sql)?;
            let rows = stmt
                .query_map(named_params! {":id": id}, |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            for (class, n) in rows {
                match class.as_str() {
                    "synthetic_implicit" => count.synthetic_implicit += n,
                    "synthetic_explicit" => count.synthetic_explicit += n,
                    _ => count.other += n,
                }
                count.rows += n;
            }
        }
        if count.rows > 0 {
            refs.insert(r.key, count);
        }
    }
    Ok(refs)
}

/// The columns whose `other`-class rows hold a subject back.
pub fn held_columns(refs: &Refs) -> Vec<&'static str> {
    refs.iter().filter(|(_, c)| c.outcome != Outcome::SetNull && c.other > 0).map(|(k, _)| *k).collect()
}

fn refs_json(refs: &Refs) -> Value {
    Value::Object(refs.iter().map(|(k, c)| ((*k).to_string(), c.to_json())).collect())
}

/// The purge class, over alias `t`: a synthetic-marked, disabled, unclaimed, unverified, unpaid,
/// unbilled tenant that is not a perf fixture.
fn purge_class() -> String {
    let reasons = ret::PURGE_DISABLED_REASONS.iter().map(|r| format!("'{r}'")).collect::<Vec<_>>().join(", ");
    format!(
        "t.origin = '{origin}' AND t.synthetic IS NOT NULL AND t.disabled = 1 \
         AND t.disabled_reason IN ({reasons}) AND t.claimed_unix IS NULL AND t.owner_verified_at IS NULL \
         AND t.paid_unix IS NULL AND t.billing_ref IS NULL AND t.namespace NOT LIKE 'perf-%'",
        origin = ret::SELECTOR_ORIGIN,
    )
}

/// When a purge-class row is due: `MAX(last seen or created, its newest lifecycle event, the policy
/// floor) + :grace`, so a row disabled before the purge shipped waits the full grace from the floor.
fn purge_due() -> String {
    "MAX(COALESCE(t.last_seen_unix, t.created_unix), \
         COALESCE((SELECT MAX(e.created_unix) FROM tenant_lifecycle_events e WHERE e.tenant_id = t.id), 0), \
         COALESCE((SELECT floor_unix FROM retention_policy WHERE table_name = 'synthetic_implicit_tenants'), 0)) \
     + :grace"
        .to_string()
}

/// What one purge stage did (or, report-only, would do).
#[derive(Debug, Default, Clone)]
pub struct PurgeStage {
    pub purged: i64,
    pub would_purge: i64,
    pub held_over: i64,
    pub held_entangled: BTreeMap<String, i64>,
    pub cross_refs: BTreeMap<String, RefCount>,
    pub vacuumed_pages: i64,
    pub freelist_pages: i64,
    pub purge_due_unix: Option<i64>,
    /// Subjects whose delete failed (logged, retried by the next sweep).
    pub errors: i64,
    /// `(subjects deleted, wall ms)` per batch.
    pub batches: Vec<(i64, i64)>,
}

impl PurgeStage {
    fn note_refs(&mut self, refs: &Refs) {
        for (k, c) in refs {
            self.cross_refs.entry((*k).to_string()).or_insert_with(|| RefCount::new(c.outcome)).add(c);
        }
    }

    fn cross_refs_json(&self) -> Value {
        Value::Object(self.cross_refs.iter().map(|(k, c)| (k.clone(), c.to_json())).collect())
    }

    /// The `prune_log` block fields.
    pub fn insert_into(&self, block: &mut serde_json::Map<String, Value>) {
        block.insert("purged".into(), json!(self.purged));
        block.insert("would_purge".into(), json!(self.would_purge));
        block.insert("held_over".into(), json!(self.held_over));
        block.insert("held_entangled".into(), json!(self.held_entangled));
        block.insert("cross_refs".into(), self.cross_refs_json());
        block.insert("vacuumed_pages".into(), json!(self.vacuumed_pages));
        block.insert("freelist_pages".into(), json!(self.freelist_pages));
        block.insert("purge_due_unix".into(), json!(self.purge_due_unix));
        block.insert("batch_wall_ms".into(), json!(self.batches.iter().map(|(_, ms)| *ms).collect::<Vec<_>>()));
        if self.errors > 0 {
            block.insert("purge_errors".into(), json!(self.errors));
        }
    }
}

/// The outcome of one subject's transaction.
enum One {
    /// Not in the class, or not due, any more: a claim, payment or `admin.tenant_enable` won.
    Changed,
    Entangled(Refs, Vec<&'static str>),
    Deleted(Refs),
}

/// `(subjects due at `now`, earliest due stamp over the class)`.
fn due_counts(conn: &Connection, now: i64, grace_secs: i64) -> Result<(i64, Option<i64>), AppError> {
    let sql = format!(
        "SELECT COUNT(CASE WHEN due <= :now THEN 1 END), MIN(due) FROM \
         (SELECT {due} AS due FROM tenants t WHERE {class})",
        due = purge_due(),
        class = purge_class(),
    );
    Ok(conn.query_row(&sql, named_params! {":now": now, ":grace": grace_secs}, |r| Ok((r.get(0)?, r.get(1)?)))?)
}

/// The next `limit` due subject ids above `after`, in id order.
fn due_ids(conn: &Connection, now: i64, grace_secs: i64, after: i64, limit: i64) -> Result<Vec<i64>, AppError> {
    let sql = format!(
        "SELECT t.id FROM tenants t WHERE {class} AND t.id > :after AND {due} <= :now ORDER BY t.id LIMIT :limit",
        due = purge_due(),
        class = purge_class(),
    );
    let mut stmt = conn.prepare(&sql)?;
    let ids = stmt
        .query_map(
            named_params! {":now": now, ":grace": grace_secs, ":after": after, ":limit": limit},
            |r| r.get(0),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ids)
}

/// The tenant `id`, while it is still a due member of the purge class.
fn still_due(conn: &Connection, id: i64, now: i64, grace_secs: i64) -> Result<Option<Tenant>, AppError> {
    let sql = format!(
        "SELECT t.namespace FROM tenants t WHERE t.id = :id AND {class} AND {due} <= :now",
        due = purge_due(),
        class = purge_class(),
    );
    let namespace: Option<String> = conn
        .prepare_cached(&sql)?
        .query_row(named_params! {":id": id, ":now": now, ":grace": grace_secs}, |r| r.get(0))
        .optional()?;
    match namespace {
        Some(ns) => Db::query_tenant_by_namespace(conn, &ns),
        None => Ok(None),
    }
}

/// What the expire stage did in one sweep.
#[derive(Debug, Default)]
struct ExpireStage {
    expired: i64,
    held_over: i64,
}

impl Db {
    /// The start of the stage clock and its budget.
    fn stage_clock() -> (Instant, std::time::Duration) {
        (Instant::now(), ret::stage_budget())
    }

    /// `true` once a `prune_log` row carries the purge block's `purge_due_unix`: only an armed
    /// sweep (window on, grace not `off`, mutex held) writes it, so the sweep after deploy reports
    /// and the next one may delete. Keyed to the synthetic block, never the external one (whose
    /// `pre_floor` carries a field of the same name).
    pub(crate) async fn purge_block_logged(&self) -> Result<bool, AppError> {
        self.with_conn(|conn| {
            Ok(conn
                .prepare(
                    "SELECT 1 FROM prune_log WHERE json_valid(deleted_json) \
                     AND json_type(deleted_json, '$.synthetic_implicit_tenants.purge_due_unix') IS NOT NULL LIMIT 1",
                )?
                .exists([])?)
        })
        .await
    }

    /// Enabled implicit synthetic unclaimed rows minted before the floor (the backlog whose idle
    /// clock starts at the floor) and, for the first armed sweep, a sample of at most `limit`.
    async fn pre_floor(&self, now: i64, limit: usize) -> Result<(i64, Vec<Value>), AppError> {
        self.with_conn(move |conn| {
            let class = synthetic_expiry_predicate(ExpiryTerms::SUBJECT);
            let floor_sql = "COALESCE((SELECT floor_unix FROM retention_policy \
                             WHERE table_name = 'synthetic_implicit_tenants'), 0)";
            let count: i64 = conn.query_row(
                &format!("SELECT COUNT(*) FROM tenants WHERE {class} AND created_unix < {floor_sql}"),
                [],
                |r| r.get(0),
            )?;
            let mut sample = Vec::new();
            if limit > 0 && count > 0 {
                let mut stmt = conn.prepare(&format!(
                    "SELECT id, display_name, created_unix, {EXPIRY_IDLE_SINCE} FROM tenants \
                     WHERE {class} AND created_unix < {floor_sql} ORDER BY id LIMIT ?1"
                ))?;
                sample = stmt
                    .query_map(params![limit as i64], |r| {
                        let idle_since: i64 = r.get(3)?;
                        Ok(json!({
                            "tenant_id": r.get::<_, i64>(0)?,
                            "display_name": r.get::<_, String>(1)?,
                            "created_unix": r.get::<_, i64>(2)?,
                            "idle_days": ((now - idle_since) / 86_400).max(0),
                        }))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
            }
            Ok((count, sample))
        })
        .await
    }

    /// The disable pass: batches of `EXPIRE_BATCH_SIZE` while the stage's own wall time is under
    /// the budget (the first batch always runs); the remainder is `held_over`.
    async fn expire_stage(
        &self,
        days: i64,
        now_unix: i64,
        deleted: &mut BTreeMap<String, i64>,
    ) -> Result<ExpireStage, AppError> {
        let policy = ret::SYNTHETIC_IMPLICIT_POLICY;
        let teardown = self.tenant_teardown.read().unwrap_or_else(|e| e.into_inner()).clone();
        let batch = ret::EXPIRE_BATCH_SIZE;
        let (started, budget) = Self::stage_clock();
        let mut stage = ExpireStage::default();
        loop {
            let ids = self.claim_idle_synthetic_tenants(now_unix, days, batch).await?;
            for id in &ids {
                if let Some(teardown) = &teardown {
                    teardown(*id).await;
                }
            }
            stage.expired += ids.len() as i64;
            if (ids.len() as i64) < batch || started.elapsed() >= budget {
                break;
            }
        }
        let (skipped_billing, would_expire, remaining) = self.synthetic_expiry_counts(now_unix, days).await?;
        stage.held_over = remaining;
        deleted.insert(format!("{policy}.expired"), stage.expired);
        deleted.insert(format!("{policy}.skipped_billing"), skipped_billing);
        deleted.insert(format!("{policy}.would_expire"), would_expire);
        deleted.insert(format!("{policy}.held_over"), stage.held_over);
        Ok(stage)
    }

    /// The expire half of one sweep: the policy's window decides whether it runs at all. Returns
    /// the policy block (`None` while the policy row is absent) and whether the purge may run
    /// after the external sweep.
    pub(crate) async fn synthetic_expire_half(
        &self,
        windows: &[(String, i64)],
        now_unix: i64,
        deleted: &mut BTreeMap<String, i64>,
    ) -> Result<Option<SyntheticSweep>, AppError> {
        let policy = ret::SYNTHETIC_IMPLICIT_POLICY;
        let Some(&(_, days)) = windows.iter().find(|(n, _)| n == policy) else {
            return Ok(None);
        };
        let mut block = serde_json::Map::new();
        if days <= 0 {
            block.insert("expire".into(), json!("off"));
            block.insert("purge".into(), json!("off"));
            return Ok(Some(SyntheticSweep { block, grace_days: None, first: false }));
        }
        let grace_days = ret::synthetic_purge_grace_days();
        if grace_days.is_none() {
            block.insert("purge".into(), json!("off"));
        }
        // The first armed sweep lists the pre-floor backlog; it is read before the stage empties it.
        let first = grace_days.is_some() && !self.purge_block_logged().await?;
        let limit = if first { ret::PRE_FLOOR_SAMPLE_LIMIT } else { 0 };
        let (pre_floor, sample) = self.pre_floor(now_unix, limit).await?;
        block.insert("pre_floor".into(), json!(pre_floor));
        if first {
            block.insert("pre_floor_sample".into(), Value::Array(sample));
        }
        let stage = self.expire_stage(days, now_unix, deleted).await?;
        block.insert("expired".into(), json!(stage.expired));
        block.insert("expire_held_over".into(), json!(stage.held_over));
        Ok(Some(SyntheticSweep { block, grace_days, first }))
    }

    /// The purge half of one sweep, after the external sweep. The first armed sweep reports and
    /// deletes nothing.
    pub(crate) async fn synthetic_purge_half(
        &self,
        sweep: &mut SyntheticSweep,
        now_unix: i64,
    ) -> Result<(), AppError> {
        let Some(grace_days) = sweep.grace_days else {
            return Ok(());
        };
        let stage = if sweep.first {
            self.report_purge(now_unix, grace_days).await?
        } else {
            self.run_purge(now_unix, grace_days).await?
        };
        stage.insert_into(&mut sweep.block);
        Ok(())
    }

    /// What `admin.prune_now {dry_run: true}` returns: no write, no sweep mutex, one read
    /// snapshot, computed even with the grace env `off` (the default grace then applies).
    pub async fn synthetic_dry_run(&self) -> Result<Value, AppError> {
        let now = now_unix();
        let windows = self.retention_windows().await?;
        let days = windows
            .iter()
            .find(|(n, _)| n == ret::SYNTHETIC_IMPLICIT_POLICY)
            .map(|(_, d)| *d)
            .unwrap_or(0);
        if days <= 0 {
            return Ok(json!({
                "would_expire": 0, "pre_floor": 0, "would_purge": 0, "purge_due_unix": null,
                "cross_refs": {}, "held_entangled": {},
            }));
        }
        let grace_days = ret::synthetic_purge_grace_days()
            .unwrap_or_else(|| ret::days_from_env(ret::SYNTHETIC_PURGE_GRACE_ENV, ret::SYNTHETIC_PURGE_GRACE_DEFAULT_DAYS));
        let stage = self.report_purge(now, grace_days).await?;
        let cutoff = now - days.saturating_mul(86_400);
        let (pre_floor, would_expire) = self
            .with_conn(move |conn| {
                let floor_sql = "COALESCE((SELECT floor_unix FROM retention_policy \
                                 WHERE table_name = 'synthetic_implicit_tenants'), 0)";
                let class = synthetic_expiry_predicate(ExpiryTerms::SUBJECT);
                let pre_floor: i64 = conn.query_row(
                    &format!("SELECT COUNT(*) FROM tenants WHERE {class} AND created_unix < {floor_sql}"),
                    [],
                    |r| r.get(0),
                )?;
                let would_expire: i64 = conn.query_row(
                    &format!("SELECT COUNT(*) FROM tenants WHERE {}", synthetic_expiry_predicate(ExpiryTerms::WOULD_EXPIRE)),
                    named_params! {":cutoff": cutoff},
                    |r| r.get(0),
                )?;
                Ok((pre_floor, would_expire))
            })
            .await?;
        Ok(json!({
            "would_expire": would_expire,
            "pre_floor": pre_floor,
            "would_purge": stage.would_purge,
            "purge_due_unix": stage.purge_due_unix,
            "cross_refs": stage.cross_refs_json(),
            "held_entangled": stage.held_entangled,
        }))
    }

    /// Counts the due subjects and their cross references on one read snapshot of a dedicated
    /// connection (a WAL reader never blocks the server's writer), deleting nothing.
    async fn report_purge(&self, now: i64, grace_days: i64) -> Result<PurgeStage, AppError> {
        let path = self.path.clone();
        let cfg = self.cfg;
        tokio::task::spawn_blocking(move || report_sync(&path, &cfg, now, grace_days.saturating_mul(86_400)))
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?
    }

    /// Deletes due subjects one immediate transaction each, in batches of `PURGE_BATCH`, while the
    /// stage's own wall time is under the budget; each batch ends in a bounded page reclaim. The
    /// stage owns a connection of its own (the `prune` role, like `retention::prune_sync`), so the
    /// server's shared connection is never held across a subject: readers are never blocked (WAL),
    /// a `host.tool_call` write waits for at most one subject's transaction, and a pause as long as
    /// the transaction just run (capped) follows each delete so a waiting writer gets in.
    async fn run_purge(&self, now: i64, grace_days: i64) -> Result<PurgeStage, AppError> {
        let grace_secs = grace_days.saturating_mul(86_400);
        let conn = self.purge_conn().await?;
        let (would_purge, due_unix) = on_conn(&conn, move |c| due_counts(c, now, grace_secs)).await?;
        let mut stage = PurgeStage { would_purge, purge_due_unix: due_unix, ..Default::default() };
        let removed = self.tenant_removed.read().unwrap_or_else(|e| e.into_inner()).clone();
        let (started, budget) = Self::stage_clock();
        let mut after = 0i64;
        let mut out_of_budget = false;
        while !out_of_budget {
            let ids = on_conn(&conn, move |c| due_ids(c, now, grace_secs, after, ret::PURGE_BATCH)).await?;
            if ids.is_empty() {
                break;
            }
            let batch_started = Instant::now();
            let purged_before = stage.purged;
            for id in ids {
                if started.elapsed() >= budget {
                    out_of_budget = true;
                    break;
                }
                after = id;
                let tx_started = Instant::now();
                let one = match purge_one(&conn, id, now, grace_secs).await {
                    Ok(one) => one,
                    Err(e) => {
                        // One subject that cannot be deleted must not stop the rest of the stage.
                        tracing::warn!(tenant_id = id, error = %e, "synthetic tenant purge failed");
                        stage.errors += 1;
                        continue;
                    }
                };
                match one {
                    One::Changed => {}
                    One::Entangled(refs, held) => {
                        stage.note_refs(&refs);
                        for col in held {
                            *stage.held_entangled.entry(col.to_string()).or_default() += 1;
                        }
                    }
                    One::Deleted(refs) => {
                        stage.note_refs(&refs);
                        stage.purged += 1;
                        if let Some(removed) = &removed {
                            removed(id).await;
                        }
                        tokio::time::sleep(tx_started.elapsed().clamp(PAUSE_MIN, PAUSE_MAX)).await;
                    }
                }
            }
            let purged_in_batch = stage.purged - purged_before;
            let (vacuumed, freelist) = on_conn(&conn, reclaim_pages).await?;
            stage.vacuumed_pages += vacuumed;
            stage.freelist_pages = freelist;
            let wall_ms = batch_started.elapsed().as_millis() as i64;
            stage.batches.push((purged_in_batch, wall_ms));
            tracing::info!(
                purged = purged_in_batch,
                held_over = stage.would_purge - stage.purged,
                wall_ms,
                vacuumed_pages = vacuumed,
                "synthetic tenant purge batch"
            );
        }
        if out_of_budget {
            stage.held_over = on_conn(&conn, move |c| Ok(due_ids(c, now, grace_secs, after, i64::MAX)?.len() as i64)).await?;
        }
        Ok(stage)
    }

    /// The purge stage's own connection.
    async fn purge_conn(&self) -> Result<Arc<Mutex<Connection>>, AppError> {
        let path = self.path.clone();
        let cfg = self.cfg;
        tokio::task::spawn_blocking(move || {
            let (conn, _audit) = open_with_role(&path, ROLE_PRUNE, &cfg)?;
            // Every statement the stage runs is reused for each subject.
            conn.set_prepared_statement_cache_capacity(256);
            Ok(Arc::new(Mutex::new(conn)))
        })
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
    }

    /// `admin.funnel`'s `tenants_synthetic_purged`: a `COUNT` of the journal rows, so it survives
    /// a restart.
    pub async fn count_tenants_synthetic_purged(&self) -> Result<i64, AppError> {
        self.with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM admin_events WHERE action = ?1",
                params![ret::SYNTHETIC_PURGE_ACTION],
                |r| r.get(0),
            )?)
        })
        .await
    }
}

/// The expire half's carry-over to the purge half.
pub(crate) struct SyntheticSweep {
    pub block: serde_json::Map<String, Value>,
    /// `None` while the purge is off (grace `off` or the idle window off).
    pub grace_days: Option<i64>,
    /// No earlier armed sweep carried `purge_due_unix`: report, delete nothing.
    pub first: bool,
}

/// Runs `f` on the stage's connection off the async runtime.
async fn on_conn<T: Send + 'static>(
    conn: &Arc<Mutex<Connection>>,
    f: impl FnOnce(&Connection) -> Result<T, AppError> + Send + 'static,
) -> Result<T, AppError> {
    let conn = conn.clone();
    tokio::task::spawn_blocking(move || {
        let guard = conn.lock().map_err(|_| AppError::Storage("purge connection poisoned".into()))?;
        f(&guard)
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?
}

/// Pause after each delete: as long as the transaction just run, within these bounds.
const PAUSE_MIN: std::time::Duration = std::time::Duration::from_millis(1);
const PAUSE_MAX: std::time::Duration = std::time::Duration::from_millis(100);

/// One subject. Read first, with no write lock: a subject that is no longer due, or that a row of
/// an `other`-class tenant names, costs the server's writers nothing. Then one immediate
/// transaction re-checks the class and the references and deletes.
async fn purge_one(conn: &Arc<Mutex<Connection>>, id: i64, now: i64, grace_secs: i64) -> Result<One, AppError> {
    let pre = on_conn(conn, move |conn| {
        if still_due(conn, id, now, grace_secs)?.is_none() {
            return Ok(Some(One::Changed));
        }
        let refs = count_cross_refs(conn, id)?;
        let held = held_columns(&refs);
        Ok((!held.is_empty()).then_some(One::Entangled(refs, held)))
    })
    .await?;
    if let Some(done) = pre {
        return Ok(done);
    }
    on_conn(conn, move |conn| {
        crate::txn::with_tx(conn, crate::txn::TxBehavior::Immediate, |conn| {
            let Some(tenant) = still_due(conn, id, now, grace_secs)? else {
                return Ok(One::Changed);
            };
            let refs = count_cross_refs(conn, id)?;
            let held = held_columns(&refs);
            if !held.is_empty() {
                return Ok(One::Entangled(refs, held));
            }
            let counts = Db::count_tenant_child_rows(conn, id)?;
            Db::delete_tenant_rows_with(conn, &tenant, &counts, ret::SYNTHETIC_PURGE_ACTION, Some(refs_json(&refs)))?;
            Ok(One::Deleted(refs))
        })
    })
    .await
}

/// `PRAGMA incremental_vacuum(PURGE_VACUUM_PAGES)` after a batch: `(pages reclaimed, freelist
/// pages left)`. Never a full `VACUUM`.
fn reclaim_pages(conn: &Connection) -> Result<(i64, i64), AppError> {
    let before: i64 = conn.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
    ret::incremental_vacuum(conn, ret::PURGE_VACUUM_PAGES)?;
    let after: i64 = conn.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
    Ok(((before - after).max(0), after))
}

/// Counts the due subjects and their cross references on one snapshot, within the stage budget.
fn report_sync(path: &Path, cfg: &DbConfig, now: i64, grace_secs: i64) -> Result<PurgeStage, AppError> {
    let (conn, _audit) = open_with_role(path, ROLE_PRUNE, cfg)?;
    crate::txn::with_tx(&conn, crate::txn::TxBehavior::Deferred, |conn| {
        let (would_purge, due_unix) = due_counts(conn, now, grace_secs)?;
        let mut stage = PurgeStage { would_purge, purge_due_unix: due_unix, ..Default::default() };
        let (started, budget) = Db::stage_clock();
        let mut after = 0i64;
        'all: loop {
            let ids = due_ids(conn, now, grace_secs, after, ret::PURGE_BATCH)?;
            if ids.is_empty() {
                break;
            }
            for id in ids {
                if started.elapsed() >= budget {
                    stage.held_over = due_ids(conn, now, grace_secs, after, i64::MAX)?.len() as i64;
                    break 'all;
                }
                after = id;
                let refs = count_cross_refs(conn, id)?;
                for col in held_columns(&refs) {
                    *stage.held_entangled.entry(col.to_string()).or_default() += 1;
                }
                stage.note_refs(&refs);
            }
        }
        Ok(stage)
    })
}
