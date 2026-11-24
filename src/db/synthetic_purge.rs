//! PRD-mcphost-synthetic-tenant-purge R1/R3/R4/R9: the synthetic purge stage --
//! class and due predicates, the cross-tenant relation registry, the
//! per-subject delete transaction and the batched, budgeted stage.
//!
//! A subject is a two-marker, disabled, unclaimed, unverified, unpaid, unbilled
//! tenant 14 idle days after the policy floor. Each subject is one immediate
//! transaction on the shared connection (a waiter gets in between subjects);
//! the stage reports `held_over` when its wall budget stops it. A subject that
//! a row of a tenant outside the synthetic class names is held, never purged.

use super::*;
use crate::retention::{
    PURGE_BATCH, PURGE_VACUUM_PAGES, SELECTOR_ORIGIN, SYNTHETIC_IMPLICIT_POLICY, SYNTHETIC_PURGE_ACTION,
    SYNTHETIC_PURGE_REASONS,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

/// Fixed-name perf fixtures (`perf-<plan>`, `src/perf.rs`) are never subjects.
pub const PERF_NAMESPACE_PREFIX: &str = "perf-";

/// Least pause between two subjects, and the pause after a subject that was
/// skipped: a caller queued on the connection runs between them instead of
/// behind the whole batch. After a delete the pause is as long as the
/// transaction was, so the stage holds the connection at most half the time.
const PURGE_YIELD_MS: u64 = 1;

/// What a purge does to the rows of one cross-tenant column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Nullable reference, set to `NULL` inside the delete transaction.
    Nulled,
    /// `NOT NULL` column with no foreign key: the rows are deleted by value.
    Delete,
    /// `NOT NULL ... ON DELETE CASCADE`: the other tenant's row goes with the subject.
    Cascade,
    /// `ON DELETE SET NULL` by schema: an author column with no counterpart column.
    SetNull,
}

impl Outcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Outcome::Nulled => "nulled",
            Outcome::Delete => "delete",
            Outcome::Cascade => "cascade",
            Outcome::SetNull => "set_null",
        }
    }
}

/// One two-sided column (§ Cascade). `pairs` selects `(s, o)` for every row that
/// names a subject `s` (`{S}` is the subject set) and belongs to another tenant
/// `o`; a `SetNull` relation selects `(s, NULL)`.
pub struct Relation {
    pub key: &'static str,
    pub outcome: Outcome,
    pub pairs: &'static str,
}

/// Every cross-tenant column a purge can reach, flat and structural. The
/// registry is the classification: `tests/tenp_ac02` derives the same set from
/// the schema, so a table added later fails by name until it is listed here.
pub const RELATIONS: &[Relation] = &[
    // -- two-sided by owner column --
    Relation {
        key: "calls.caller_tenant_id",
        outcome: Outcome::Nulled,
        pairs: "SELECT caller_tenant_id AS s, tenant_id AS o FROM calls \
                WHERE caller_tenant_id IN {S} AND tenant_id != caller_tenant_id",
    },
    Relation {
        key: "runs.caller_tenant_id",
        outcome: Outcome::Nulled,
        pairs: "SELECT caller_tenant_id AS s, tenant_id AS o FROM runs \
                WHERE caller_tenant_id IN {S} AND tenant_id != caller_tenant_id",
    },
    Relation {
        key: "tenants.invited_by_tenant_id",
        outcome: Outcome::Nulled,
        pairs: "SELECT invited_by_tenant_id AS s, id AS o FROM tenants \
                WHERE invited_by_tenant_id IN {S} AND id != invited_by_tenant_id",
    },
    Relation {
        key: "tenants.scratch_of",
        outcome: Outcome::Nulled,
        pairs: "SELECT scratch_of AS s, id AS o FROM tenants WHERE scratch_of IN {S} AND id != scratch_of",
    },
    Relation {
        key: "usage_daily.caller_tenant_id",
        outcome: Outcome::Delete,
        pairs: "SELECT caller_tenant_id AS s, tenant_id AS o FROM usage_daily \
                WHERE caller_tenant_id IN {S} AND tenant_id != caller_tenant_id",
    },
    Relation {
        key: "blocks.blocked_tenant_id",
        outcome: Outcome::Cascade,
        pairs: "SELECT blocked_tenant_id AS s, tenant_id AS o FROM blocks \
                WHERE blocked_tenant_id IN {S} AND tenant_id != blocked_tenant_id",
    },
    Relation {
        key: "contacts.contact_tenant_id",
        outcome: Outcome::Cascade,
        pairs: "SELECT contact_tenant_id AS s, tenant_id AS o FROM contacts \
                WHERE contact_tenant_id IN {S} AND tenant_id != contact_tenant_id",
    },
    Relation {
        key: "muted.muted_tenant_id",
        outcome: Outcome::Cascade,
        pairs: "SELECT muted_tenant_id AS s, tenant_id AS o FROM muted \
                WHERE muted_tenant_id IN {S} AND tenant_id != muted_tenant_id",
    },
    Relation {
        key: "tool_version_watermarks.caller_tenant_id",
        outcome: Outcome::Cascade,
        pairs: "SELECT caller_tenant_id AS s, tenant_id AS o FROM tool_version_watermarks \
                WHERE caller_tenant_id IN {S} AND tenant_id != caller_tenant_id",
    },
    Relation {
        key: "tool_caller_limits.caller_tenant_id",
        outcome: Outcome::Cascade,
        pairs: "SELECT caller_tenant_id AS s, tenant_id AS o FROM tool_caller_limits \
                WHERE caller_tenant_id IN {S} AND tenant_id != caller_tenant_id",
    },
    // -- two-sided by pair (either direction) --
    Relation {
        key: "contact_requests.from_tenant_id",
        outcome: Outcome::Cascade,
        pairs: "SELECT from_tenant_id AS s, to_tenant_id AS o FROM contact_requests \
                WHERE from_tenant_id IN {S} AND to_tenant_id != from_tenant_id",
    },
    Relation {
        key: "contact_requests.to_tenant_id",
        outcome: Outcome::Cascade,
        pairs: "SELECT to_tenant_id AS s, from_tenant_id AS o FROM contact_requests \
                WHERE to_tenant_id IN {S} AND from_tenant_id != to_tenant_id",
    },
    Relation {
        key: "tasks.poster_tenant_id",
        outcome: Outcome::Cascade,
        pairs: "SELECT poster_tenant_id AS s, recipient_tenant_id AS o FROM tasks \
                WHERE poster_tenant_id IN {S} AND recipient_tenant_id != poster_tenant_id",
    },
    Relation {
        key: "tasks.recipient_tenant_id",
        outcome: Outcome::Cascade,
        pairs: "SELECT recipient_tenant_id AS s, poster_tenant_id AS o FROM tasks \
                WHERE recipient_tenant_id IN {S} AND poster_tenant_id != recipient_tenant_id",
    },
    Relation {
        key: "invite_joins.inviter_tenant_id",
        outcome: Outcome::Delete,
        pairs: "SELECT inviter_tenant_id AS s, invitee_tenant_id AS o FROM invite_joins \
                WHERE inviter_tenant_id IN {S} AND invitee_tenant_id != inviter_tenant_id",
    },
    Relation {
        key: "invite_joins.invitee_tenant_id",
        outcome: Outcome::Cascade,
        pairs: "SELECT invitee_tenant_id AS s, inviter_tenant_id AS o FROM invite_joins \
                WHERE invitee_tenant_id IN {S} AND inviter_tenant_id != invitee_tenant_id",
    },
    // -- structural: the subject owns the parent row, the child's tenant column is the other side --
    Relation {
        key: "groups.owner_tenant_id/group_members.member_tenant_id",
        outcome: Outcome::Cascade,
        pairs: "SELECT g.owner_tenant_id AS s, m.member_tenant_id AS o FROM groups g \
                JOIN group_members m ON m.group_id = g.id \
                WHERE g.owner_tenant_id IN {S} AND m.member_tenant_id != g.owner_tenant_id",
    },
    Relation {
        key: "groups.owner_tenant_id/channels.created_by",
        outcome: Outcome::Cascade,
        pairs: "SELECT g.owner_tenant_id AS s, c.created_by AS o FROM groups g \
                JOIN channels c ON c.group_id = g.id \
                WHERE g.owner_tenant_id IN {S} AND c.created_by IS NOT NULL AND c.created_by != g.owner_tenant_id",
    },
    Relation {
        key: "groups.owner_tenant_id/channel_posts.from_tenant_id",
        outcome: Outcome::Cascade,
        pairs: "SELECT g.owner_tenant_id AS s, p.from_tenant_id AS o FROM groups g \
                JOIN channels c ON c.group_id = g.id JOIN channel_posts p ON p.channel_id = c.id \
                WHERE g.owner_tenant_id IN {S} AND p.from_tenant_id IS NOT NULL AND p.from_tenant_id != g.owner_tenant_id",
    },
    Relation {
        key: "groups.owner_tenant_id/channel_cursors.tenant_id",
        outcome: Outcome::Cascade,
        pairs: "SELECT g.owner_tenant_id AS s, k.tenant_id AS o FROM groups g \
                JOIN channels c ON c.group_id = g.id JOIN channel_cursors k ON k.channel_id = c.id \
                WHERE g.owner_tenant_id IN {S} AND k.tenant_id != g.owner_tenant_id",
    },
    Relation {
        key: "orgs.owner_tenant_id/org_members.tenant_id",
        outcome: Outcome::Cascade,
        pairs: "SELECT o.owner_tenant_id AS s, m.tenant_id AS o FROM orgs o \
                JOIN org_members m ON m.org_id = o.id \
                WHERE o.owner_tenant_id IN {S} AND m.tenant_id != o.owner_tenant_id",
    },
    Relation {
        key: "orgs.owner_tenant_id/invites.inviter_tenant_id",
        outcome: Outcome::Cascade,
        pairs: "SELECT o.owner_tenant_id AS s, i.inviter_tenant_id AS o FROM orgs o \
                JOIN invites i ON i.org_id = o.id \
                WHERE o.owner_tenant_id IN {S} AND i.inviter_tenant_id != o.owner_tenant_id",
    },
    Relation {
        key: "orgs.owner_tenant_id/invite_joins.invitee_tenant_id",
        outcome: Outcome::Cascade,
        pairs: "SELECT o.owner_tenant_id AS s, j.invitee_tenant_id AS o FROM orgs o \
                JOIN invites i ON i.org_id = o.id JOIN invite_joins j ON j.invite_id = i.id \
                WHERE o.owner_tenant_id IN {S} AND j.invitee_tenant_id != o.owner_tenant_id",
    },
    Relation {
        key: "invites.inviter_tenant_id/invite_joins.invitee_tenant_id",
        outcome: Outcome::Cascade,
        pairs: "SELECT i.inviter_tenant_id AS s, j.invitee_tenant_id AS o FROM invites i \
                JOIN invite_joins j ON j.invite_id = i.id \
                WHERE i.inviter_tenant_id IN {S} AND j.invitee_tenant_id != i.inviter_tenant_id",
    },
    // -- SET NULL by schema, no counterpart column: counted, never held --
    Relation {
        key: "channel_posts.from_tenant_id",
        outcome: Outcome::SetNull,
        pairs: "SELECT from_tenant_id AS s, NULL AS o FROM channel_posts WHERE from_tenant_id IN {S}",
    },
    Relation {
        key: "channels.created_by",
        outcome: Outcome::SetNull,
        pairs: "SELECT created_by AS s, NULL AS o FROM channels WHERE created_by IN {S}",
    },
    Relation {
        key: "messages.from_tenant_id",
        outcome: Outcome::SetNull,
        pairs: "SELECT from_tenant_id AS s, NULL AS o FROM messages WHERE from_tenant_id IN {S}",
    },
    Relation {
        key: "threads.created_by",
        outcome: Outcome::SetNull,
        pairs: "SELECT created_by AS s, NULL AS o FROM threads WHERE created_by IN {S}",
    },
    Relation {
        key: "signup_events.tenant_id",
        outcome: Outcome::SetNull,
        pairs: "SELECT tenant_id AS s, NULL AS o FROM signup_events WHERE tenant_id IN {S}",
    },
];

/// Structural members whose only INSERT path writes the parent's own tenant, so
/// no other tenant's row is reached (`tests/tenp_ac02` asserts it after the
/// fixture's writes). Keyed `"<parent.column>/<child.column>"`.
pub const SAME_TENANT: &[&str] = &[
    "notifies.tenant_id/notify_receipts.tenant_id",
    "oauth_grants.tenant_id/oauth_jti_denylist.tenant_id",
];

/// The `tenants` columns of the other side's class, decided by the owner's own
/// non-disabled terms, never by `origin` alone.
const OTHER_CLASS: &str = "CASE WHEN o.origin = 'synthetic' AND o.synthetic IS NOT NULL AND o.claimed_unix IS NULL \
     AND o.owner_verified_at IS NULL AND o.paid_unix IS NULL AND o.billing_ref IS NULL \
     THEN CASE WHEN o.signup_source = 'implicit' THEN 'synthetic_implicit' ELSE 'synthetic_explicit' END \
     ELSE 'other' END";

/// R1 class, written once: `p` is the table alias (`"t."`) or empty.
pub fn purge_class(p: &str) -> String {
    let reasons = SYNTHETIC_PURGE_REASONS.iter().map(|r| format!("'{r}'")).collect::<Vec<_>>().join(", ");
    format!(
        "{p}origin = '{origin}' AND {p}synthetic IS NOT NULL AND {p}disabled = 1 \
         AND {p}disabled_reason IN ({reasons}) AND {p}claimed_unix IS NULL AND {p}owner_verified_at IS NULL \
         AND {p}paid_unix IS NULL AND {p}billing_ref IS NULL AND {p}namespace NOT LIKE '{perf}%'",
        origin = SELECTOR_ORIGIN,
        perf = PERF_NAMESPACE_PREFIX,
    )
}

/// R1 due stamp: `MAX(last seen or created, latest lifecycle event, floor) +
/// grace`, so a row disabled before this PRD (no event) waits the full grace
/// from the floor. Binds `:grace_secs`.
pub fn purge_due_stamp(p: &str) -> String {
    format!(
        "(MAX(COALESCE({p}last_seen_unix, {p}created_unix, 0), \
         COALESCE((SELECT MAX(e.created_unix) FROM tenant_lifecycle_events e WHERE e.tenant_id = {p}id), 0), \
         COALESCE((SELECT floor_unix FROM retention_policy WHERE table_name = '{policy}'), 0)) + :grace_secs)",
        policy = SYNTHETIC_IMPLICIT_POLICY,
    )
}

/// The subjects a cross-reference query ranges over.
#[derive(Clone, Copy, Debug)]
pub(crate) enum SubjectSet {
    One(i64),
    Due { now: i64, grace_secs: i64 },
}

/// The cross-reference query of every relation as one statement (a compound
/// `SELECT` of one arm per relation: `k` the relation's index in [`RELATIONS`],
/// `s` the subject, `cls` the other tenant's class, `n` the row count), written
/// once for a single subject and once for the whole due set.
struct Plans {
    one: String,
    due: String,
}

static PLANS: LazyLock<Plans> = LazyLock::new(|| {
    let one = "(:id)".to_string();
    let due = format!("(SELECT t.id FROM tenants t WHERE {} AND {} <= :now)", purge_class("t."), purge_due_stamp("t."));
    let build = |set: &str| {
        RELATIONS
            .iter()
            .enumerate()
            .map(|(k, relation)| {
                let pairs = relation.pairs.replace("{S}", set);
                match relation.outcome {
                    Outcome::SetNull => {
                        format!("SELECT {k} AS k, s, 'set_null' AS cls, COUNT(*) AS n FROM ({pairs}) GROUP BY s")
                    }
                    _ => format!(
                        "SELECT {k} AS k, r.s AS s, {OTHER_CLASS} AS cls, COUNT(*) AS n FROM ({pairs}) r \
                         JOIN tenants o ON o.id = r.o GROUP BY r.s, cls"
                    ),
                }
            })
            .collect::<Vec<_>>()
            .join(" UNION ALL ")
    };
    Plans { one: build(&one), due: build(&due) }
});

/// Summed rows of one column, by the other tenant's class.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RefCount {
    pub outcome: Outcome,
    pub implicit: i64,
    pub explicit: i64,
    pub other: i64,
    pub rows: i64,
}

impl RefCount {
    fn new(outcome: Outcome) -> Self {
        Self { outcome, implicit: 0, explicit: 0, other: 0, rows: 0 }
    }

    fn to_json(self) -> Value {
        if self.outcome == Outcome::SetNull {
            return json!({"outcome": self.outcome.as_str(), "rows": self.rows, "other": null});
        }
        json!({
            "outcome": self.outcome.as_str(),
            "synthetic_implicit": self.implicit,
            "synthetic_explicit": self.explicit,
            "other": self.other,
        })
    }
}

/// Cross-tenant rows naming a set of subjects.
#[derive(Clone, Debug, Default)]
pub(crate) struct RefTally {
    /// Rows by column (only columns with at least one row).
    pub refs: BTreeMap<&'static str, RefCount>,
    /// Per column, the number of subjects an `other`-class tenant's row names.
    pub held: BTreeMap<&'static str, i64>,
    /// Subjects with at least one such row.
    pub entangled: BTreeSet<i64>,
}

impl RefTally {
    fn add(&mut self, other: &RefTally) {
        for (key, c) in &other.refs {
            let into = self.refs.entry(key).or_insert_with(|| RefCount::new(c.outcome));
            into.implicit += c.implicit;
            into.explicit += c.explicit;
            into.other += c.other;
            into.rows += c.rows;
        }
        for (key, n) in &other.held {
            *self.held.entry(key).or_default() += n;
        }
        self.entangled.extend(other.entangled.iter().copied());
    }

    pub(crate) fn cross_refs_json(&self) -> Value {
        Value::Object(self.refs.iter().map(|(k, c)| ((*k).to_string(), c.to_json())).collect())
    }

    pub(crate) fn held_json(&self) -> Value {
        Value::Object(self.held.iter().map(|(k, n)| ((*k).to_string(), json!(n))).collect())
    }

    /// The columns an `other`-class row names this single subject through.
    fn entangled_columns(&self) -> Vec<&'static str> {
        self.held.keys().copied().collect()
    }
}

/// Counts every cross-tenant row naming `set`, split by the other tenant's class.
pub(crate) fn cross_ref_tally(conn: &Connection, set: SubjectSet) -> Result<RefTally, AppError> {
    let mut tally = RefTally::default();
    let sql = match set {
        SubjectSet::One(_) => &PLANS.one,
        SubjectSet::Due { .. } => &PLANS.due,
    };
    let mut stmt = conn.prepare_cached(sql)?;
    let mut bind = |name: &str, value: i64| -> Result<(), AppError> {
        if let Some(i) = stmt.parameter_index(name)? {
            stmt.raw_bind_parameter(i, value)?;
        }
        Ok(())
    };
    match set {
        SubjectSet::One(id) => bind(":id", id)?,
        SubjectSet::Due { now, grace_secs } => {
            bind(":now", now)?;
            bind(":grace_secs", grace_secs)?;
        }
    }
    let mut held_subjects: BTreeMap<usize, BTreeSet<i64>> = BTreeMap::new();
    let mut rows = stmt.raw_query();
    while let Some(row) = rows.next()? {
        let k: usize = row.get::<_, i64>(0)? as usize;
        let subject: i64 = row.get(1)?;
        let class: String = row.get(2)?;
        let n: i64 = row.get(3)?;
        let relation = &RELATIONS[k];
        let c = tally.refs.entry(relation.key).or_insert_with(|| RefCount::new(relation.outcome));
        c.rows += n;
        match class.as_str() {
            "synthetic_implicit" => c.implicit += n,
            "synthetic_explicit" => c.explicit += n,
            "other" => {
                c.other += n;
                held_subjects.entry(k).or_default().insert(subject);
            }
            _ => {}
        }
    }
    for (k, subjects) in held_subjects {
        *tally.held.entry(RELATIONS[k].key).or_default() += subjects.len() as i64;
        tally.entangled.extend(subjects);
    }
    Ok(tally)
}

/// How one subject's delete transaction ended.
pub(crate) enum SubjectOutcome {
    /// Re-enabled, claimed, verified, paid, billed, or already deleted since it was listed.
    Gone,
    /// The class holds but the due stamp has not passed.
    NotDue,
    /// An `other`-class tenant's row names it: nothing was written.
    Held(RefTally),
    Purged { tenant_id: i64, tally: RefTally },
}

/// What one stage run reports.
#[derive(Default)]
struct StageTally {
    purged: i64,
    failed: i64,
    first_error: Option<String>,
    tally: RefTally,
    held_subjects: i64,
    vacuumed_pages: i64,
    batch_wall_ms: Vec<u64>,
}

/// A report over every due subject (the report-only sweep and `dry_run`).
pub(crate) struct PurgeReport {
    pub would_purge: i64,
    pub tally: RefTally,
    pub purge_due_unix: Option<i64>,
}

/// [`Db::purge_report`] over a connection already inside a read transaction.
fn purge_report_sync(conn: &Connection, now: i64, grace_days: i64) -> Result<PurgeReport, AppError> {
    let grace_secs = grace_days.saturating_mul(86_400);
    let class = purge_class("t.");
    let due_stamp = purge_due_stamp("t.");
    let due: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM tenants t WHERE {class} AND {due_stamp} <= :now"),
        named_params! {":now": now, ":grace_secs": grace_secs},
        |r| r.get(0),
    )?;
    let purge_due_unix: Option<i64> = conn.query_row(
        &format!("SELECT MIN({due_stamp}) FROM tenants t WHERE {class}"),
        named_params! {":grace_secs": grace_secs},
        |r| r.get(0),
    )?;
    let tally = cross_ref_tally(conn, SubjectSet::Due { now, grace_secs })?;
    let would_purge = due - tally.entangled.len() as i64;
    Ok(PurgeReport { would_purge, tally, purge_due_unix })
}

impl Db {
    /// The R1 gate: `true` once any earlier `prune_log` row carries
    /// `purge_due_unix` in this policy's block. The external policy's block
    /// has its own `pre_floor.purge_due_unix`, so the test reads the exact path.
    pub(crate) async fn purge_gate_open(&self) -> Result<bool, AppError> {
        self.with_conn(|conn| {
            Ok(conn
                .prepare(
                    "SELECT 1 FROM prune_log WHERE json_valid(deleted_json) \
                     AND json_type(deleted_json, ?1) IS NOT NULL LIMIT 1",
                )?
                .exists(params![format!("$.{SYNTHETIC_IMPLICIT_POLICY}.purge_due_unix")])?)
        })
        .await
    }

    /// Counts and cross-references of every due subject under one read transaction.
    pub(crate) async fn purge_report(&self, now: i64, grace_days: i64) -> Result<PurgeReport, AppError> {
        self.with_conn(move |conn| {
            crate::txn::with_tx(conn, crate::txn::TxBehavior::Deferred, |conn| purge_report_sync(conn, now, grace_days))
        })
        .await
    }

    /// R4 `dry_run`: the numbers a sweep would log, from one deferred read
    /// transaction. It takes no sweep lock, writes nothing and ignores the grace
    /// env being `off` (the default grace stands in), so the numbers exist while
    /// a deploy runs with the purge stopped.
    pub(crate) async fn synthetic_dry_run(&self, now: i64, window_days: i64) -> Result<Value, AppError> {
        let grace_days = crate::retention::synthetic_purge_grace_days()
            .unwrap_or(crate::retention::SYNTHETIC_PURGE_GRACE_DEFAULT_DAYS);
        let cutoff = now - window_days.max(0).saturating_mul(86_400);
        let expiring = window_days > 0 && !crate::retention::env_is_off(crate::retention::SYNTHETIC_IMPLICIT_ENV);
        self.with_conn(move |conn| {
            crate::txn::with_tx(conn, crate::txn::TxBehavior::Deferred, |conn| {
                let would_expire: i64 = if expiring {
                    conn.query_row(
                        &format!(
                            "SELECT COUNT(*) FROM tenants WHERE {}",
                            super::synthetic_expiry_predicate(super::ExpiryTerms::WOULD_EXPIRE)
                        ),
                        named_params! {":cutoff": cutoff},
                        |r| r.get(0),
                    )?
                } else {
                    0
                };
                let pre_floor: i64 = conn.query_row(
                    &format!(
                        "SELECT COUNT(*) FROM tenants WHERE {} AND created_unix < \
                         COALESCE((SELECT floor_unix FROM retention_policy WHERE table_name = '{SYNTHETIC_IMPLICIT_POLICY}'), 0)",
                        super::expiry_class(false)
                    ),
                    [],
                    |r| r.get(0),
                )?;
                let report = purge_report_sync(conn, now, grace_days)?;
                Ok(json!({
                    "would_expire": would_expire,
                    "pre_floor": pre_floor,
                    "would_purge": report.would_purge,
                    "purge_due_unix": report.purge_due_unix,
                    "cross_refs": report.tally.cross_refs_json(),
                    "held_entangled": report.tally.held_json(),
                }))
            })
        })
        .await
    }

    /// One subject's delete transaction: re-check the class and due stamp, count
    /// the cross-tenant rows, hold on any `other`, else delete.
    fn purge_subject_sync(conn: &Connection, id: i64, now: i64, grace_secs: i64) -> Result<SubjectOutcome, AppError> {
        crate::txn::with_tx(conn, crate::txn::TxBehavior::Immediate, |conn| {
            let class = purge_class("t.");
            let due_stamp = purge_due_stamp("t.");
            let row: Option<(String, i64)> = conn
                .query_row(
                    &format!("SELECT t.namespace, {due_stamp} FROM tenants t WHERE t.id = :id AND {class}"),
                    named_params! {":id": id, ":grace_secs": grace_secs},
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((namespace, due)) = row else {
                return Ok(SubjectOutcome::Gone);
            };
            if due > now {
                return Ok(SubjectOutcome::NotDue);
            }
            let tally = cross_ref_tally(conn, SubjectSet::One(id))?;
            if !tally.entangled.is_empty() {
                return Ok(SubjectOutcome::Held(tally));
            }
            let detail = json!({"cross_refs": tally.cross_refs_json()});
            let Some((tenant, _)) =
                Self::delete_tenant_as_sync(conn, &namespace, SYNTHETIC_PURGE_ACTION, Some(detail))?
            else {
                return Ok(SubjectOutcome::Gone);
            };
            Ok(SubjectOutcome::Purged { tenant_id: tenant.id, tally })
        })
    }

    /// Up to `limit` due subject ids after `after`, ascending.
    async fn purge_candidates(&self, now: i64, grace_secs: i64, after: i64, limit: i64) -> Result<Vec<i64>, AppError> {
        self.with_conn(move |conn| {
            let sql = format!(
                "SELECT t.id FROM tenants t WHERE {} AND {} <= :now AND t.id > :after ORDER BY t.id LIMIT :limit",
                purge_class("t."),
                purge_due_stamp("t."),
            );
            let mut stmt = conn.prepare_cached(&sql)?;
            let ids = stmt
                .query_map(
                    named_params! {":now": now, ":grace_secs": grace_secs, ":after": after, ":limit": limit},
                    |r| r.get::<_, i64>(0),
                )?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ids)
        })
        .await
    }

    async fn purge_remaining(&self, now: i64, grace_secs: i64, after: i64) -> Result<i64, AppError> {
        self.with_conn(move |conn| {
            let sql = format!(
                "SELECT COUNT(*) FROM tenants t WHERE {} AND {} <= :now AND t.id > :after",
                purge_class("t."),
                purge_due_stamp("t."),
            );
            Ok(conn.query_row(
                &sql,
                named_params! {":now": now, ":grace_secs": grace_secs, ":after": after},
                |r| r.get(0),
            )?)
        })
        .await
    }

    /// `PRAGMA incremental_vacuum(PURGE_VACUUM_PAGES)`: the pages returned and
    /// the freelist left. Never a full `VACUUM`.
    async fn purge_vacuum_batch(&self) -> Result<(i64, i64), AppError> {
        self.with_conn(|conn| {
            let before: i64 = conn.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
            let mut stmt = conn.prepare(&format!("PRAGMA incremental_vacuum({PURGE_VACUUM_PAGES})"))?;
            let mut rows = stmt.query([])?;
            while rows.next()?.is_some() {}
            drop(rows);
            drop(stmt);
            let after: i64 = conn.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
            Ok(((before - after).max(0), after))
        })
        .await
    }

    /// The armed purge stage: batches of [`PURGE_BATCH`] subjects while the
    /// stage's own wall time is under the budget; the remainder is `held_over`.
    async fn run_purge_stage(&self, now: i64, grace_days: i64, block: &mut serde_json::Map<String, Value>) -> Result<(), AppError> {
        let grace_secs = grace_days.saturating_mul(86_400);
        let cleanup = self.tenant_purge_cleanup.read().unwrap_or_else(|e| e.into_inner()).clone();
        let budget = Duration::from_secs(crate::retention::stage_budget_secs());
        let stage_start = Instant::now();
        let due_at_start = self.purge_report_due_count(now, grace_secs).await?;
        let mut st = StageTally::default();
        let mut after = 0i64;
        let mut exhausted = false;
        loop {
            let batch_start = Instant::now();
            let ids = self.purge_candidates(now, grace_secs, after, PURGE_BATCH).await?;
            if ids.is_empty() {
                exhausted = true;
                break;
            }
            let full = ids.len() as i64 == PURGE_BATCH;
            let (mut purged_here, mut held_here) = (0i64, 0i64);
            for &id in &ids {
                after = id;
                let subject_start = Instant::now();
                let outcome = self
                    .with_conn(move |conn| Self::purge_subject_sync(conn, id, now, grace_secs))
                    .await;
                let held_connection = subject_start.elapsed();
                match outcome {
                    Ok(SubjectOutcome::Purged { tenant_id, tally }) => {
                        st.purged += 1;
                        purged_here += 1;
                        st.tally.add(&tally);
                        if let Some(cleanup) = &cleanup {
                            cleanup(tenant_id).await;
                        }
                    }
                    Ok(SubjectOutcome::Held(tally)) => {
                        st.held_subjects += 1;
                        held_here += 1;
                        tracing::info!(subject = id, columns = ?tally.entangled_columns(), "synthetic purge: held, named by a row of another tenant");
                        st.tally.add(&tally);
                    }
                    Ok(SubjectOutcome::Gone | SubjectOutcome::NotDue) => {}
                    Err(e) => {
                        st.failed += 1;
                        tracing::warn!(subject = id, error = %e, "synthetic purge: subject failed");
                        st.first_error.get_or_insert_with(|| format!("{e}"));
                    }
                }
                // The purge holds the connection at most half the time: a caller queued
                // behind a subject's transaction gets in for as long as it ran.
                tokio::time::sleep(held_connection.max(Duration::from_millis(PURGE_YIELD_MS))).await;
            }
            let (vacuumed, _) = self.purge_vacuum_batch().await?;
            st.vacuumed_pages += vacuumed;
            let wall_ms = batch_start.elapsed().as_millis() as u64;
            st.batch_wall_ms.push(wall_ms);
            tracing::info!(purged = purged_here, held = held_here, wall_ms, vacuumed_pages = vacuumed, "synthetic purge batch");
            if !full {
                exhausted = true;
                break;
            }
            if stage_start.elapsed() >= budget {
                break;
            }
        }
        let held_over = if exhausted { 0 } else { self.purge_remaining(now, grace_secs, after).await? };
        let freelist = self.freelist_pages().await?;
        let purge_due_unix = self.earliest_purge_due(grace_secs).await?;
        block.insert("purged".into(), json!(st.purged));
        block.insert("would_purge".into(), json!(due_at_start));
        block.insert("held_over".into(), json!(held_over));
        block.insert("held_entangled".into(), st.tally.held_json());
        block.insert("cross_refs".into(), st.tally.cross_refs_json());
        block.insert("vacuumed_pages".into(), json!(st.vacuumed_pages));
        block.insert("freelist_pages".into(), json!(freelist));
        block.insert("purge_due_unix".into(), json!(purge_due_unix));
        block.insert("batch_wall_ms".into(), json!(st.batch_wall_ms));
        if st.failed > 0 {
            block.insert("failed".into(), json!(st.failed));
            block.insert("first_error".into(), json!(st.first_error));
        }
        Ok(())
    }

    async fn purge_report_due_count(&self, now: i64, grace_secs: i64) -> Result<i64, AppError> {
        self.purge_remaining(now, grace_secs, 0).await
    }

    async fn freelist_pages(&self) -> Result<i64, AppError> {
        self.with_conn(|conn| Ok(conn.query_row("PRAGMA freelist_count", [], |r| r.get(0))?)).await
    }

    async fn earliest_purge_due(&self, grace_secs: i64) -> Result<Option<i64>, AppError> {
        self.with_conn(move |conn| {
            Ok(conn.query_row(
                &format!("SELECT MIN({}) FROM tenants t WHERE {}", purge_due_stamp("t."), purge_class("t.")),
                named_params! {":grace_secs": grace_secs},
                |r| r.get(0),
            )?)
        })
        .await
    }

    /// The purge half of one sweep's policy block (R1). Writes `purge: "off"`
    /// and no `purge_due_unix` while the grace env or the idle-days env is off;
    /// otherwise the first sweep that finds no armed `prune_log` row only
    /// reports, and later sweeps delete.
    pub(super) async fn sweep_purge_block(
        &self,
        windows: &[(String, i64)],
        started: i64,
        gate_open: bool,
        block: &mut serde_json::Map<String, Value>,
    ) -> Result<(), AppError> {
        let Some(&(_, days)) = windows.iter().find(|(n, _)| n == SYNTHETIC_IMPLICIT_POLICY) else {
            return Ok(());
        };
        if days <= 0 || crate::retention::env_is_off(crate::retention::SYNTHETIC_IMPLICIT_ENV) {
            block.insert("expire".into(), json!("off"));
            block.insert("purge".into(), json!("off"));
            return Ok(());
        }
        let Some(grace_days) = crate::retention::synthetic_purge_grace_days() else {
            block.insert("purge".into(), json!("off"));
            return Ok(());
        };
        if !gate_open {
            let report = self.purge_report(started, grace_days).await?;
            let freelist = self.freelist_pages().await?;
            block.insert("purged".into(), json!(0));
            block.insert("would_purge".into(), json!(report.would_purge));
            block.insert("held_over".into(), json!(0));
            block.insert("held_entangled".into(), report.tally.held_json());
            block.insert("cross_refs".into(), report.tally.cross_refs_json());
            block.insert("vacuumed_pages".into(), json!(0));
            block.insert("freelist_pages".into(), json!(freelist));
            block.insert("purge_due_unix".into(), json!(report.purge_due_unix));
            return Ok(());
        }
        self.run_purge_stage(started, grace_days, block).await
    }
}
