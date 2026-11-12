//! PRD-mcphost-queue-primitive-v2 R1/R3/R4/R6: the `queues` / `queue_items`
//! statements behind `host.queue.*` and the dispatcher tick.
//!
//! Every multi-row decision (the quota check beside the insert, the claim
//! beside the run insert, a state change beside its drained check) is ONE
//! [`Db::with_conn`] closure, which the single-writer connection serialises:
//! no second caller can interleave, so no lock or retry is needed.

use super::*;
use crate::events::QueueState;
use crate::run_enums::RunStatus;

/// One `queues` row.
#[derive(Debug, Clone)]
pub struct QueueRow {
    pub id: i64,
    pub tenant_id: i64,
    pub name: String,
    pub consumer_tool: String,
    pub concurrency: i64,
    pub state: String,
    pub created_at_ms: i64,
    pub drained_at_ms: Option<i64>,
}

/// One `GROUP BY status` read of a queue's items (one snapshot, so the four
/// counts always sum to the items the queue holds).
#[derive(Debug, Clone, Copy, Default)]
pub struct QueueCounts {
    pub pending: i64,
    pub running: i64,
    pub done: i64,
    pub failed: i64,
    /// Items started: `running`, `done` or `failed` (R5 `attempts_total`).
    pub attempts_total: i64,
    /// The smallest `pushed_at_ms` among `pending` items.
    pub oldest_pending_ms: Option<i64>,
}

/// One `queue_items` row.
#[derive(Debug, Clone)]
pub struct QueueItemRow {
    pub id: i64,
    pub queue_id: i64,
    pub idx: i64,
    pub item_json: String,
    pub status: String,
    pub run_id: Option<String>,
    pub attempts: i64,
    pub last_error: Option<String>,
    pub ended_at_ms: Option<i64>,
}

/// A row for `Db::queue_test_insert_item`.
#[doc(hidden)]
pub struct SeedItem {
    pub item_json: String,
    pub status: &'static str,
    pub run_id: Option<String>,
    pub ended_at_ms: Option<i64>,
}

/// What `host.queue.create` asks for.
pub struct QueueSpec {
    pub name: String,
    pub consumer_tool: String,
    pub concurrency: i64,
}

/// One state verb: the states it applies to, where it moves the queue, and
/// whether it then settles `draining` into `drained`.
#[derive(Clone, Copy)]
struct MoveRule {
    from: &'static [QueueState],
    to: QueueState,
    settle: bool,
}

/// What [`Db::queue_create`] did.
pub enum QueueCreate {
    Created(QueueRow),
    Duplicate,
    /// The tenant already holds `used` queues, the plan's ceiling.
    Quota { used: i64 },
}

/// What [`Db::queue_push`] did.
pub enum QueuePush {
    Pushed { first_idx: Option<i64>, last_idx: Option<i64> },
    /// Not `active` or `paused`: the queue's state, for `queue_not_accepting`.
    NotAccepting(String),
    /// Pending plus running would pass the ceiling; nothing was inserted.
    Quota { used: i64 },
    Gone,
}

/// What [`Db::queue_claim_and_start`] did with one pending item.
#[derive(Debug, PartialEq, Eq)]
pub enum Claim {
    Started,
    /// Another tick got the item first, or it is no longer pending.
    Lost,
    /// A cap (the queue's concurrency, the tenant's `jobs_concurrent`) or the
    /// queue's state holds the item back for now.
    NoRoom,
}

/// What a state verb found.
pub enum QueueMove {
    Moved(QueueRow),
    /// The queue is in a state the verb does not apply to.
    Refused(QueueRow),
    Gone,
}

/// The `runs` row a claimed item starts as.
pub struct QueueRun {
    pub run_id: String,
    pub tool_name: String,
    pub args_json: String,
    pub deadline_s: i64,
}

/// Runs `f` as one transaction (a nested savepoint inside the dry-run scope): all
/// of its writes land or none do.
fn atomic<T>(conn: &Connection, f: impl FnOnce(&Connection) -> Result<T, AppError>) -> Result<T, AppError> {
    crate::txn::with_tx(conn, crate::txn::TxBehavior::Immediate, f)
}

const COLS: &str = "id, tenant_id, name, consumer_tool, concurrency, state, created_at_ms, drained_at_ms";

fn queue_row(r: &Row<'_>) -> rusqlite::Result<QueueRow> {
    Ok(QueueRow {
        id: r.get(0)?,
        tenant_id: r.get(1)?,
        name: r.get(2)?,
        consumer_tool: r.get(3)?,
        concurrency: r.get(4)?,
        state: r.get(5)?,
        created_at_ms: r.get(6)?,
        drained_at_ms: r.get(7)?,
    })
}

/// A `'queued','running',...` SQL list from run statuses (spelled by the enum, never literals).
fn run_status_list(keep: impl Fn(RunStatus) -> bool) -> String {
    RunStatus::ALL
        .iter()
        .copied()
        .filter(|s| keep(*s))
        .map(|s| format!("'{}'", s.as_str()))
        .collect::<Vec<_>>()
        .join(",")
}

fn counts_of(conn: &Connection, queue_id: i64) -> Result<QueueCounts, AppError> {
    let mut out = QueueCounts::default();
    let mut stmt = conn.prepare_cached("SELECT status, COUNT(*) FROM queue_items WHERE queue_id = ?1 GROUP BY status")?;
    let rows = stmt.query_map(params![queue_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    for row in rows {
        let (status, n) = row?;
        match status.as_str() {
            "pending" => out.pending = n,
            "running" => out.running = n,
            "done" => out.done = n,
            "failed" => out.failed = n,
            _ => {}
        }
    }
    out.attempts_total = out.running + out.done + out.failed;
    out.oldest_pending_ms = conn.query_row(
        "SELECT MIN(pushed_at_ms) FROM queue_items WHERE queue_id = ?1 AND status = 'pending'",
        params![queue_id],
        |r| r.get(0),
    )?;
    Ok(out)
}

/// `draining` → `drained` once nothing is pending or running; `scope` narrows to one queue.
fn settle_drained(conn: &Connection, now_ms: i64, only_queue: Option<i64>) -> Result<usize, AppError> {
    let n = conn.execute(
        "UPDATE queues SET state = ?1, drained_at_ms = ?2 \
         WHERE state = ?3 AND (?4 IS NULL OR id = ?4) \
         AND NOT EXISTS (SELECT 1 FROM queue_items i WHERE i.queue_id = queues.id AND i.status IN ('pending', 'running'))",
        params![QueueState::Drained.as_str(), now_ms, QueueState::Draining.as_str(), only_queue],
    )?;
    Ok(n)
}

impl Db {
    /// R2: inserts a queue unless the name is taken or the tenant holds `queues_max` already.
    pub async fn queue_create(
        &self,
        tenant_id: i64,
        spec: QueueSpec,
        queues_max: i64,
    ) -> Result<QueueCreate, AppError> {
        let QueueSpec { name, consumer_tool, concurrency } = spec;
        let now_ms = crate::state::now_unix_ms();
        self.with_conn(move |conn| {
            let taken: bool = conn
                .prepare_cached("SELECT 1 FROM queues WHERE tenant_id = ?1 AND name = ?2")?
                .exists(params![tenant_id, name])?;
            if taken {
                return Ok(QueueCreate::Duplicate);
            }
            let used: i64 = conn.query_row("SELECT COUNT(*) FROM queues WHERE tenant_id = ?1", params![tenant_id], |r| r.get(0))?;
            if used >= queues_max {
                return Ok(QueueCreate::Quota { used });
            }
            conn.execute(
                "INSERT INTO queues (tenant_id, name, consumer_tool, concurrency, state, created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![tenant_id, name, consumer_tool, concurrency, QueueState::Active.as_str(), now_ms],
            )?;
            let row = conn.query_row(&format!("SELECT {COLS} FROM queues WHERE id = ?1"), params![conn.last_insert_rowid()], queue_row)?;
            Ok(QueueCreate::Created(row))
        })
        .await
    }

    pub async fn queue_get(&self, tenant_id: i64, name: String) -> Result<Option<QueueRow>, AppError> {
        self.with_conn(move |conn| {
            conn.query_row(&format!("SELECT {COLS} FROM queues WHERE tenant_id = ?1 AND name = ?2"), params![tenant_id, name], queue_row)
                .optional()
                .map_err(AppError::from)
        })
        .await
    }

    /// A queue and its item counts in one closure (one snapshot).
    pub async fn queue_get_with_counts(&self, tenant_id: i64, name: String) -> Result<Option<(QueueRow, QueueCounts)>, AppError> {
        self.with_conn(move |conn| {
            let Some(row) = conn
                .query_row(&format!("SELECT {COLS} FROM queues WHERE tenant_id = ?1 AND name = ?2"), params![tenant_id, name], queue_row)
                .optional()?
            else {
                return Ok(None);
            };
            let counts = counts_of(conn, row.id)?;
            Ok(Some((row, counts)))
        })
        .await
    }

    /// R5 `host.queue.list`: every queue of the tenant, name order, with counts.
    pub async fn queue_list(&self, tenant_id: i64) -> Result<Vec<(QueueRow, QueueCounts)>, AppError> {
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(&format!("SELECT {COLS} FROM queues WHERE tenant_id = ?1 ORDER BY name"))?;
            let rows = stmt.query_map(params![tenant_id], queue_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
            rows.into_iter().map(|q| counts_of(conn, q.id).map(|c| (q, c))).collect()
        })
        .await
    }

    /// Every item of one queue in `idx` order (the push order).
    pub async fn queue_item_rows(&self, queue_id: i64) -> Result<Vec<QueueItemRow>, AppError> {
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, queue_id, idx, item_json, status, run_id, attempts, last_error, ended_at_ms \
                 FROM queue_items WHERE queue_id = ?1 ORDER BY idx",
            )?;
            let rows = stmt
                .query_map(params![queue_id], |r| {
                    Ok(QueueItemRow {
                        id: r.get(0)?,
                        queue_id: r.get(1)?,
                        idx: r.get(2)?,
                        item_json: r.get(3)?,
                        status: r.get(4)?,
                        run_id: r.get(5)?,
                        attempts: r.get(6)?,
                        last_error: r.get(7)?,
                        ended_at_ms: r.get(8)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await
    }

    /// R3: inserts already-validated items in push order. The state check, the
    /// ceiling check (the tenant's pending plus running across its queues) and the
    /// insert are one closure, so a concurrent drain or push cannot slip between.
    pub async fn queue_push(&self, tenant_id: i64, queue_id: i64, items: Vec<String>, items_max: i64) -> Result<QueuePush, AppError> {
        let now_ms = crate::state::now_unix_ms();
        self.with_conn(move |conn| {
            let Some(state) = conn
                .query_row("SELECT state FROM queues WHERE id = ?1 AND tenant_id = ?2", params![queue_id, tenant_id], |r| r.get::<_, String>(0))
                .optional()?
            else {
                return Ok(QueuePush::Gone);
            };
            if state != QueueState::Active.as_str() && state != QueueState::Paused.as_str() {
                return Ok(QueuePush::NotAccepting(state));
            }
            let used: i64 = conn.query_row(
                "SELECT COUNT(*) FROM queue_items i JOIN queues q ON q.id = i.queue_id \
                 WHERE q.tenant_id = ?1 AND i.status IN ('pending', 'running')",
                params![tenant_id],
                |r| r.get(0),
            )?;
            if used + items.len() as i64 > items_max {
                return Ok(QueuePush::Quota { used });
            }
            if items.is_empty() {
                return Ok(QueuePush::Pushed { first_idx: None, last_idx: None });
            }
            let next: i64 = conn.query_row("SELECT COALESCE(MAX(idx) + 1, 0) FROM queue_items WHERE queue_id = ?1", params![queue_id], |r| r.get(0))?;
            atomic(conn, |conn| {
                let mut ins = conn.prepare_cached(
                    "INSERT INTO queue_items (queue_id, idx, item_json, status, attempts, pushed_at_ms) VALUES (?1, ?2, ?3, 'pending', 0, ?4)",
                )?;
                for (n, item) in items.iter().enumerate() {
                    ins.execute(params![queue_id, next + n as i64, item, now_ms])?;
                }
                Ok(())
            })?;
            Ok(QueuePush::Pushed { first_idx: Some(next), last_idx: Some(next + items.len() as i64 - 1) })
        })
        .await
    }

    /// R6 pause: `active` → `paused`. Already `paused` answers as moved.
    pub async fn queue_pause(&self, tenant_id: i64, name: String) -> Result<QueueMove, AppError> {
        self.queue_move(tenant_id, name, MoveRule { from: &[QueueState::Active, QueueState::Paused], to: QueueState::Paused, settle: false }).await
    }

    /// R6 resume: `paused` → `active`. Already `active` answers as moved.
    pub async fn queue_resume(&self, tenant_id: i64, name: String) -> Result<QueueMove, AppError> {
        self.queue_move(tenant_id, name, MoveRule { from: &[QueueState::Active, QueueState::Paused], to: QueueState::Active, settle: false }).await
    }

    /// R6 drain: `active|paused` → `draining`, straight on to `drained` when
    /// nothing is pending or running. An already `draining`/`drained` queue answers as moved.
    pub async fn queue_drain(&self, tenant_id: i64, name: String) -> Result<QueueMove, AppError> {
        let from = &[QueueState::Active, QueueState::Paused, QueueState::Draining, QueueState::Drained];
        self.queue_move(tenant_id, name, MoveRule { from, to: QueueState::Draining, settle: true }).await
    }

    async fn queue_move(&self, tenant_id: i64, name: String, rule: MoveRule) -> Result<QueueMove, AppError> {
        let MoveRule { from, to, settle } = rule;
        let now_ms = crate::state::now_unix_ms();
        self.with_conn(move |conn| {
            let Some(row) = conn
                .query_row(&format!("SELECT {COLS} FROM queues WHERE tenant_id = ?1 AND name = ?2"), params![tenant_id, name], queue_row)
                .optional()?
            else {
                return Ok(QueueMove::Gone);
            };
            if !from.iter().any(|s| s.as_str() == row.state) {
                return Ok(QueueMove::Refused(row));
            }
            // A drain never walks back: draining/drained stay as they are.
            let keep = settle && (row.state == QueueState::Draining.as_str() || row.state == QueueState::Drained.as_str());
            if !keep {
                conn.execute("UPDATE queues SET state = ?1 WHERE id = ?2", params![to.as_str(), row.id])?;
            }
            if settle {
                settle_drained(conn, now_ms, Some(row.id))?;
            }
            let row = conn.query_row(&format!("SELECT {COLS} FROM queues WHERE id = ?1"), params![row.id], queue_row)?;
            Ok(QueueMove::Moved(row))
        })
        .await
    }

    /// R8 delete: removes a `drained` or `paused` queue and its items.
    /// `Refused` carries an `active` or `draining` queue untouched.
    pub async fn queue_delete(&self, tenant_id: i64, name: String) -> Result<QueueMove, AppError> {
        self.with_conn(move |conn| {
            let Some(row) = conn
                .query_row(&format!("SELECT {COLS} FROM queues WHERE tenant_id = ?1 AND name = ?2"), params![tenant_id, name], queue_row)
                .optional()?
            else {
                return Ok(QueueMove::Gone);
            };
            if row.state != QueueState::Drained.as_str() && row.state != QueueState::Paused.as_str() {
                return Ok(QueueMove::Refused(row));
            }
            atomic(conn, |conn| {
                conn.execute("DELETE FROM queue_items WHERE queue_id = ?1", params![row.id])?;
                conn.execute("DELETE FROM queues WHERE id = ?1", params![row.id])?;
                Ok(())
            })?;
            Ok(QueueMove::Moved(row))
        })
        .await
    }

    // ---- dispatcher ----------------------------------------------------

    /// One tick's bookkeeping, in one closure: items whose run ended settle to
    /// `done`/`failed` (R4: `last_error` is the run's `error_class`); a `running`
    /// item whose run row is missing returns to `pending`; a `draining` queue that
    /// emptied becomes `drained`. Returns the number of rows changed.
    pub async fn queue_reconcile(&self) -> Result<usize, AppError> {
        let now_ms = crate::state::now_unix_ms();
        let terminal = run_status_list(RunStatus::is_terminal);
        self.with_conn(move |conn| {
            let ended = conn.execute(
                &format!(
                    "UPDATE queue_items SET \
                       status = CASE WHEN r.status = '{done}' THEN 'done' ELSE 'failed' END, \
                       last_error = CASE WHEN r.status = '{done}' THEN NULL ELSE COALESCE(r.error_class, r.status) END, \
                       ended_at_ms = ?1 \
                     FROM runs r WHERE queue_items.run_id = r.id AND queue_items.status = 'running' AND r.status IN ({terminal})",
                    done = RunStatus::Done.as_str(),
                ),
                params![now_ms],
            )?;
            let orphaned = conn.execute(
                "UPDATE queue_items SET status = 'pending', run_id = NULL, attempts = 0 \
                 WHERE status = 'running' AND NOT EXISTS (SELECT 1 FROM runs r WHERE r.id = queue_items.run_id)",
                [],
            )?;
            let settled = settle_drained(conn, now_ms, None)?;
            Ok(ended + orphaned + settled)
        })
        .await
    }

    /// Queues the dispatcher may deliver from (`active` and `draining`) that
    /// hold a pending item, with their pending items in `idx` order (at most `concurrency` each).
    pub async fn queue_dispatch_candidates(&self) -> Result<Vec<(QueueRow, Vec<(i64, String)>)>, AppError> {
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {COLS} FROM queues WHERE state IN (?1, ?2) \
                 AND EXISTS (SELECT 1 FROM queue_items i WHERE i.queue_id = queues.id AND i.status = 'pending') ORDER BY id"
            ))?;
            let queues = stmt
                .query_map(params![QueueState::Active.as_str(), QueueState::Draining.as_str()], queue_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut out = Vec::with_capacity(queues.len());
            for q in queues {
                let mut items = conn.prepare_cached("SELECT id, item_json FROM queue_items WHERE queue_id = ?1 AND status = 'pending' ORDER BY idx LIMIT ?2")?;
                let pending = items
                    .query_map(params![q.id, q.concurrency.max(1)], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                out.push((q, pending));
            }
            Ok(out)
        })
        .await
    }

    /// R4's claim: `UPDATE ... SET status = 'running', run_id = ? WHERE id = ? AND status = 'pending'`,
    /// guarded by the queue's own state and both caps, and the `runs` insert in
    /// the SAME closure -- so two ticks cannot claim one item, an item is never
    /// `running` without its run row, and neither cap can be passed by a race.
    pub async fn queue_claim_and_start(&self, item_id: i64, queue: QueueRow, jobs_concurrent: i64, run: QueueRun) -> Result<Claim, AppError> {
        let in_flight = run_status_list(|s| matches!(s, RunStatus::Queued | RunStatus::Running));
        self.with_conn(move |conn| {
            let still_pending: bool = conn
                .prepare_cached("SELECT 1 FROM queue_items WHERE id = ?1 AND status = 'pending'")?
                .exists(params![item_id])?;
            if !still_pending {
                return Ok(Claim::Lost);
            }
            atomic(conn, |conn| {
                let claimed = conn.execute(
                    &format!(
                        "UPDATE queue_items SET status = 'running', run_id = ?1, attempts = 1 \
                         WHERE id = ?2 AND status = 'pending' \
                         AND EXISTS (SELECT 1 FROM queues q WHERE q.id = ?3 AND q.state IN (?4, ?5)) \
                         AND (SELECT COUNT(*) FROM queue_items w WHERE w.queue_id = ?3 AND w.status = 'running') < ?6 \
                         AND (SELECT COUNT(*) FROM runs r WHERE r.tenant_id = ?7 AND r.status IN ({in_flight})) < ?8"
                    ),
                    params![
                        run.run_id,
                        item_id,
                        queue.id,
                        QueueState::Active.as_str(),
                        QueueState::Draining.as_str(),
                        queue.concurrency,
                        queue.tenant_id,
                        jobs_concurrent
                    ],
                )?;
                if claimed == 0 {
                    return Ok(Claim::NoRoom);
                }
                conn.execute(
                    "INSERT INTO runs (id, tenant_id, tool_name, trigger, trigger_ref, status, deadline_s, attempt, args_json, manual, test_run) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8, 0, 0)",
                    params![
                        run.run_id,
                        queue.tenant_id,
                        run.tool_name,
                        crate::run_enums::RunTrigger::Queue.as_str(),
                        queue.id.to_string(),
                        RunStatus::Queued.as_str(),
                        run.deadline_s,
                        run.args_json
                    ],
                )?;
                Ok(Claim::Started)
            })
        })
        .await
    }

    /// An item the dispatcher cannot start at all (its tool is gone, its arguments no
    /// longer fit the schema) ends `failed` with the reason, never retried.
    pub async fn queue_item_fail(&self, item_id: i64, reason: String) -> Result<(), AppError> {
        let now_ms = crate::state::now_unix_ms();
        self.with_conn(move |conn| {
            conn.execute(
                "UPDATE queue_items SET status = 'failed', last_error = ?1, ended_at_ms = ?2 WHERE id = ?3 AND status = 'pending'",
                params![reason, now_ms, item_id],
            )?;
            Ok(())
        })
        .await
    }

    /// Test seam: stores one item in a chosen state, e.g. `running` with a run id
    /// that has no `runs` row (what a crash between claim and insert would leave),
    /// or `done` with an old `ended_at_ms` (what the retention window prunes).
    #[doc(hidden)]
    pub async fn queue_test_insert_item(&self, queue_id: i64, seed: SeedItem) -> Result<(), AppError> {
        let now_ms = crate::state::now_unix_ms();
        self.with_conn(move |conn| {
            let next: i64 = conn.query_row("SELECT COALESCE(MAX(idx) + 1, 0) FROM queue_items WHERE queue_id = ?1", params![queue_id], |r| r.get(0))?;
            let attempts = i64::from(seed.status != "pending");
            conn.execute(
                "INSERT INTO queue_items (queue_id, idx, item_json, status, run_id, attempts, pushed_at_ms, ended_at_ms) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![queue_id, next, seed.item_json, seed.status, seed.run_id, attempts, now_ms, seed.ended_at_ms],
            )?;
            Ok(())
        })
        .await
    }
}
