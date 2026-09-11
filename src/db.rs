//! SQLite storage. One dedicated blocking task services every query
//! (`spawn_blocking` around a mutex-guarded [`rusqlite::Connection`]) so a
//! slow query never stalls the async runtime, per the PRD's technical
//! considerations.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;
use serde_json::{Value, json};

use crate::errors::AppError;

const MIGRATION_0001: &str = include_str!("../migrations/0001_init.sql");
const MIGRATION_0002: &str = include_str!("../migrations/0002_tenant_last_tool_change.sql");
const MIGRATION_0003: &str = include_str!("../migrations/0003_registry.sql");
const MIGRATION_0004: &str = include_str!("../migrations/0004_calls_resource_usage.sql");
const MIGRATION_0005: &str = include_str!("../migrations/0005_cascade_delete.sql");
const MIGRATION_0006: &str = include_str!("../migrations/0006_billing.sql");
const MIGRATION_0007: &str = include_str!("../migrations/0007_metering.sql");
const MIGRATION_0008: &str = include_str!("../migrations/0008_synthetic.sql");
const MIGRATION_0009: &str = include_str!("../migrations/0009_call_outcome.sql");
const MIGRATION_0010: &str = include_str!("../migrations/0010_tenant_attribution.sql");
const MIGRATION_0011: &str = include_str!("../migrations/0011_tenant_state.sql");
const MIGRATION_0012: &str = include_str!("../migrations/0012_provenance.sql");
const MIGRATION_0013: &str = include_str!("../migrations/0013_sharing.sql");
const MIGRATION_0014: &str = include_str!("../migrations/0014_runs.sql");
const MIGRATION_0015: &str = include_str!("../migrations/0015_triggers.sql");
const MIGRATION_0016: &str = include_str!("../migrations/0016_runs_manual.sql");

/// Shared by every query that selects a whole tenant row, so the column
/// list and [`tenant_from_row`] stay in lockstep with each other.
const TENANT_COLUMNS: &str = "id, namespace, display_name, key_hash, created_at, disabled, \
    last_tool_change_unix, namespace_verified, registry_namespace, plan, plan_since, billing_ref, \
    stripe_customer_id, synthetic, source_class, client_name, client_version, classified_by, \
    created_unix, origin, origin_detail";

fn tenant_from_row(r: &Row) -> rusqlite::Result<Tenant> {
    Ok(Tenant {
        id: r.get(0)?,
        namespace: r.get(1)?,
        display_name: r.get(2)?,
        key_hash: r.get(3)?,
        created_at: r.get(4)?,
        disabled: r.get::<_, i64>(5)? != 0,
        last_tool_change_unix: r.get(6)?,
        namespace_verified: r.get::<_, i64>(7)? != 0,
        registry_namespace: r.get(8)?,
        plan: r.get(9)?,
        plan_since: r.get(10)?,
        billing_ref: r.get(11)?,
        stripe_customer_id: r.get(12)?,
        synthetic: r.get(13)?,
        source_class: r.get(14)?,
        client_name: r.get(15)?,
        client_version: r.get(16)?,
        classified_by: r.get(17)?,
        created_unix: r.get(18)?,
        origin: r.get(19)?,
        origin_detail: r.get(20)?,
    })
}

/// Shared by every query that selects a whole tools row, so the column
/// list and [`tool_from_row`] stay in lockstep with each other -- same
/// convention as [`TENANT_COLUMNS`]/[`tenant_from_row`] above
/// (PRD-mcphost-sharing migration 0013).
const TOOL_COLUMNS: &str = "id, tenant_id, name, kind, spec, created_at, visibility, \
    share_description, shared_unix, shared_group, unshared_by";

fn tool_from_row(r: &Row) -> rusqlite::Result<ToolRow> {
    let spec_text: String = r.get(4)?;
    Ok(ToolRow {
        id: r.get(0)?,
        tenant_id: r.get(1)?,
        name: r.get(2)?,
        kind: r.get(3)?,
        spec: serde_json::from_str(&spec_text).unwrap_or(Value::Null),
        created_at: r.get(5)?,
        visibility: r.get(6)?,
        share_description: r.get(7)?,
        shared_unix: r.get(8)?,
        shared_group: r.get(9)?,
        unshared_by: r.get(10)?,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct Tenant {
    pub id: i64,
    pub namespace: String,
    pub display_name: String,
    pub key_hash: String,
    pub created_at: String,
    pub disabled: bool,
    /// unix seconds of the last publish OR remove for this tenant's tools;
    /// drives `tools/list`'s ttlMs cache hint (AC18). See migration 0002.
    pub last_tool_change_unix: i64,
    /// Set by `admin.tenant_verify_namespace` (AC19). The verification
    /// METHOD (DNS/HTTP) is out of scope here -- this is just the
    /// admin-set boolean outcome. See migration 0003.
    pub namespace_verified: bool,
    /// The reverse-DNS-style domain namespace `admin.tenant_verify_namespace`
    /// recorded alongside `namespace_verified`; `None` until then. Used as
    /// the registry `server.json`'s `name` field.
    pub registry_namespace: Option<String>,
    /// PRD-grand-loop-billing: `free` until a webhook (or
    /// `admin.plan_set`) says otherwise. See migration 0006.
    pub plan: String,
    /// When `plan` last changed via billing (webhook `checkout.session.completed`
    /// or `invoice.paid`, or `admin.plan_set`); `None` for a tenant that
    /// has never been anything but its migration-time default.
    pub plan_since: Option<String>,
    /// The payment processor's own id for this tenant (a Stripe customer
    /// or subscription id) -- the only processor identifier stored
    /// (technical considerations), used to resolve `invoice.paid` /
    /// `customer.subscription.deleted` / `invoice.payment_failed` events
    /// back to a tenant when they carry no `client_reference_id`.
    pub billing_ref: Option<String>,
    /// PRD-mcphost-metered-overage migration 0007: the Stripe *customer* id
    /// (never a subscription id), set only from a `checkout.session.completed`
    /// event's own `customer` field -- distinct from [`Self::billing_ref`],
    /// which falls back to `subscription` when `customer` is absent and so
    /// isn't reliable as the meter-events `payload[stripe_customer_id]`.
    pub stripe_customer_id: Option<String>,
    /// PRD-mcphost-synthetic-flag migration 0008: a free-form label
    /// (`synthorg:<run_id>`, `operator`, ...) set at signup from the
    /// `x-mcphost-synthetic` header, or later by `admin.tenant_set_synthetic`
    /// / `admin.tenants_set_synthetic`. `None` is "unlabeled" -- a real
    /// tenant, or a synthetic one from before this column existed. Metadata
    /// only: never consulted by plan/quota/billing/sandbox logic, and never
    /// surfaced in a tenant-facing response (only `admin.*` tools and
    /// `/healthz` read it).
    pub synthetic: Option<String>,
    /// PRD-mcphost-tenant-attribution migration 0010: this host's own
    /// derived `loopback`/`fleet`/`external` read of where the signup came
    /// from (`state::classify_source_class`'s output, stored as text --
    /// `None` only for a row somehow missed by both live classification
    /// and the migration's backfill, which should never happen after
    /// `migrate()` runs). `/healthz`'s `tenants_real` and `mcphost funnel`
    /// both key off this, not `synthetic`.
    pub source_class: Option<String>,
    /// The MCP `initialize` request's `clientInfo.name`, captured at
    /// signup or on first authenticated call after (requirement 2).
    /// `None` until either happens.
    pub client_name: Option<String>,
    /// `clientInfo.version`, captured alongside `client_name`.
    pub client_version: Option<String>,
    /// `"backfill"` for a tenant `migrate_0010_tenant_attribution`
    /// reclassified from before this column existed (AC3); `None` for
    /// every tenant classified live at signup.
    pub classified_by: Option<String>,
    /// `created_at` (an opaque `"unix:<secs>.<nanos>"` string) as plain
    /// epoch seconds, for `mcphost funnel`'s `--since` filter and
    /// signup-to-first-call latency. `None` only for a row the backfill
    /// couldn't parse (never true for a `created_at` this crate wrote).
    pub created_unix: Option<i64>,
    /// PRD-mcphost-provenance-audit requirement 1: the two-way real/synthetic
    /// verdict every metrics surface reports, derived at write time via
    /// `state::derive_origin` -- `'synthetic'` or `'external'`, never NULL
    /// (migration 0012).
    pub origin: String,
    /// The synthorg run-id / key-class / source_class value that justified
    /// `origin`'s verdict, or `None` for a signup with no explicit marker.
    pub origin_detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolRow {
    pub id: i64,
    pub tenant_id: i64,
    pub name: String,
    pub kind: String,
    pub spec: Value,
    pub created_at: String,
    /// PRD-mcphost-sharing migration 0013: `'private'` (default),
    /// `'group'`, or `'public'` -- the switch the cross-tenant arm of
    /// `handler.rs`'s `call_tool` reads to decide whether `<ns>.<name>`
    /// resolves for a caller outside `tenant_id`'s own namespace.
    pub visibility: String,
    /// The catalog-facing blurb set by `host.tool_share`; `None` until
    /// shared (or after `host.tool_unshare`/`admin.tool_unshare`).
    pub share_description: Option<String>,
    /// Unix seconds of the last `host.tool_share` call; `None` while
    /// private.
    pub shared_unix: Option<i64>,
    /// The group name this tool is shared to, only meaningful when
    /// `visibility == "group"`.
    pub shared_group: Option<String>,
    /// `"admin"` when `admin.tool_unshare` most recently unshared this
    /// tool; `None` otherwise (AC9).
    pub unshared_by: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct UsageStats {
    pub calls: i64,
    pub errors: i64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    /// PRD-mcphost-call-limits-honest requirement 7 (AC8): how many of
    /// `errors` in this window were `error_class = 'capacity'` -- a tenant
    /// hitting its own per-tenant admission cap (or the host-wide one)
    /// sees this directly rather than having to infer it from raw
    /// `host.tool_logs` lines.
    pub capacity_refusals: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolUsage {
    pub namespace: String,
    pub tool_name: String,
    pub stats: UsageStats,
}

/// PRD-mcphost-tenant-delete requirement 1/2: what a single tenant's
/// cascade removes, or (for the batch call's dry run) would remove.
/// `registry_documents` is cascaded too but isn't part of the wire
/// response shape the PRD specifies, so it isn't counted here.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct TenantDeleteCounts {
    pub tools_removed: i64,
    pub secrets_removed: i64,
    pub calls_removed: i64,
    pub logs_removed: i64,
}

/// PRD-mcphost-provenance-audit requirement 1: how many rows migration
/// 0012's one-shot backfill reclassified into each bucket, per table --
/// journaled at migration time (`migrate_0012_provenance`) and returned
/// directly by [`Db::backfill_provenance`] for AC3's test to assert
/// against.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct ProvenanceBackfillCounts {
    pub tenants_synthetic: i64,
    pub tenants_external: i64,
    pub signup_events_synthetic: i64,
    pub signup_events_external: i64,
    pub calls_synthetic: i64,
    pub calls_external: i64,
}

/// PRD-mcphost-provenance-audit requirement 4: a single row in
/// `admin_audit`, exposed read-only via `admin.audit_log`. Distinct from
/// `admin_events` (migration 0005, delete-only, no actor identity) --
/// `admin_audit` is this PRD's general-purpose, actor-tracked log every
/// admin-bearer mutation appends to centrally in
/// `handler::dispatch_admin_tool`, not just tenant deletion.
#[derive(Debug, Clone, Serialize)]
pub struct AdminAuditRow {
    pub id: i64,
    pub actor_key_id: String,
    pub action: String,
    pub target: Option<String>,
    pub detail: Option<String>,
    pub created_unix: i64,
}

/// PRD-mcphost-runs-and-jobs P0 requirement 1: one row of the `runs`
/// ledger, read back by `host.runs.get`/`list`/`admin.runs` and the
/// executor's own leasing/finalizing. `progress_json`/`result_ref`/
/// `error_class`/`started_unix`/`finished_unix`/`duration_ms`/`deadline_s`/
/// `trigger_ref`/`caller_tenant_id`/`purged_unix` are all nullable --
/// exactly the migration's own column shape, not narrowed here, so a
/// caller reading a `queued` row (nothing has started yet) doesn't need a
/// separate row shape from a `done` one.
#[derive(Debug, Clone, Serialize)]
pub struct RunRow {
    pub id: String,
    pub tenant_id: i64,
    pub tool_name: String,
    pub trigger: String,
    pub trigger_ref: Option<String>,
    pub caller_tenant_id: Option<i64>,
    pub status: String,
    pub progress_json: Option<String>,
    pub result_ref: Option<String>,
    pub error_class: Option<String>,
    pub started_unix: Option<i64>,
    pub finished_unix: Option<i64>,
    pub duration_ms: Option<i64>,
    pub deadline_s: Option<i64>,
    pub attempt: i64,
    pub purged_unix: Option<i64>,
    pub args_json: Option<String>,
    /// PRD-mcphost-schedules P1 requirement 7 / migration 0016: `true` only
    /// for a run `host.trigger.fire` created directly (AC10's "marked
    /// manual: true"); `false` for every run the scheduler tick itself
    /// enqueues, and for every pre-existing `call`/`job` trigger kind.
    pub manual: bool,
}

const RUN_COLUMNS: &str = "id, tenant_id, tool_name, trigger, trigger_ref, caller_tenant_id, \
    status, progress_json, result_ref, error_class, started_unix, finished_unix, duration_ms, \
    deadline_s, attempt, purged_unix, args_json, manual";

fn run_row_from_row(r: &Row) -> rusqlite::Result<RunRow> {
    Ok(RunRow {
        id: r.get(0)?,
        tenant_id: r.get(1)?,
        tool_name: r.get(2)?,
        trigger: r.get(3)?,
        trigger_ref: r.get(4)?,
        caller_tenant_id: r.get(5)?,
        status: r.get(6)?,
        progress_json: r.get(7)?,
        result_ref: r.get(8)?,
        error_class: r.get(9)?,
        started_unix: r.get(10)?,
        finished_unix: r.get(11)?,
        duration_ms: r.get(12)?,
        deadline_s: r.get(13)?,
        attempt: r.get(14)?,
        purged_unix: r.get(15)?,
        args_json: r.get(16)?,
        manual: r.get::<_, i64>(17)? != 0,
    })
}

/// PRD-mcphost-runs-and-jobs requirement 7: `host.usage`'s `jobs` block --
/// counts and total wall-clock seconds for this tenant's `trigger='job'`
/// runs finalized within the window, grouped by terminal status.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct JobsUsage {
    pub done: i64,
    pub error: i64,
    pub timeout: i64,
    pub cancelled: i64,
    pub seconds: i64,
}

/// PRD-mcphost-schedules P0 requirement 5: `host.usage`'s `scheduled`
/// block -- same shape as [`JobsUsage`] plus `skipped` (AC5's overlap
/// outcome, which a plain job never produces).
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct ScheduledUsage {
    pub done: i64,
    pub error: i64,
    pub timeout: i64,
    pub cancelled: i64,
    pub skipped: i64,
    pub seconds: i64,
}

/// One `triggers` row (migration 0015): `config_json` and `config_hash`
/// are stored, not parsed, columns -- deserializing the schedule
/// expression/args/tz out of `config_json` is `triggers.rs`'s job (the
/// same "row shape here, business logic in the RPC module" split
/// [`RunRow`] and `runs.rs` already use).
#[derive(Debug, Clone, Serialize)]
pub struct TriggerRow {
    pub id: String,
    pub tenant_id: i64,
    pub tool_name: String,
    pub kind: String,
    pub config_json: String,
    pub enabled: bool,
    pub created_unix: i64,
    pub next_unix: Option<i64>,
    pub last_run_id: Option<String>,
    pub last_fired_unix: Option<i64>,
}

const TRIGGER_COLUMNS: &str = "id, tenant_id, tool_name, kind, config_json, enabled, \
    created_unix, next_unix, last_run_id, last_fired_unix";

fn trigger_row_from_row(r: &Row) -> rusqlite::Result<TriggerRow> {
    Ok(TriggerRow {
        id: r.get(0)?,
        tenant_id: r.get(1)?,
        tool_name: r.get(2)?,
        kind: r.get(3)?,
        config_json: r.get(4)?,
        enabled: r.get::<_, i64>(5)? != 0,
        created_unix: r.get(6)?,
        next_unix: r.get(7)?,
        last_run_id: r.get(8)?,
        last_fired_unix: r.get(9)?,
    })
}

fn admin_audit_row_from_row(r: &Row) -> rusqlite::Result<AdminAuditRow> {
    Ok(AdminAuditRow {
        id: r.get(0)?,
        actor_key_id: r.get(1)?,
        action: r.get(2)?,
        target: r.get(3)?,
        detail: r.get(4)?,
        created_unix: r.get(5)?,
    })
}

impl TenantDeleteCounts {
    pub fn accumulate(&mut self, other: &TenantDeleteCounts) {
        self.tools_removed += other.tools_removed;
        self.secrets_removed += other.secrets_removed;
        self.calls_removed += other.calls_removed;
        self.logs_removed += other.logs_removed;
    }
}

/// PRD-grand-loop-billing: one row to insert into `billing_events`
/// (migration 0006). `tenant_id: None` for an event this host couldn't (or
/// deliberately didn't) resolve to a tenant -- an unknown event type, or a
/// `.mode_mismatch`-suffixed one.
#[derive(Debug, Clone)]
pub struct BillingEventInsert {
    pub event_id: String,
    pub event_type: String,
    pub tenant_id: Option<i64>,
    pub plan: Option<String>,
    pub amount_cents: Option<i64>,
    pub currency: Option<String>,
    pub mode: String,
    pub payload_sha256: String,
}

/// A `billing_events` row read back for `admin.billing_ledger` (AC9), with
/// `tenant_id` resolved to the tenant's namespace (`None` when the event
/// carried no tenant).
#[derive(Debug, Clone, Serialize)]
pub struct BillingEventRow {
    pub event_id: String,
    pub event_type: String,
    pub tenant: Option<String>,
    pub plan: Option<String>,
    pub amount_cents: Option<i64>,
    pub currency: Option<String>,
    pub mode: String,
    pub received_at: String,
}

fn now_rfc3339() -> String {
    // No chrono dependency: a stable, sortable, human-readable stamp is all
    // any AC needs (`created_at`/`started_at` are opaque strings to callers).
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("unix:{}.{:09}", dur.as_secs(), dur.subsec_nanos())
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The inverse of [`now_rfc3339`]'s `"unix:<secs>.<nanos>"` shape --
/// `None` for anything that doesn't match (a row from before this format
/// existed isn't a real case here: every `created_at` this crate has ever
/// written used this shape). PRD-mcphost-tenant-attribution migration
/// 0010's backfill and `mcphost funnel` both need `created_at` as an
/// integer, and re-parsing an opaque string at every funnel run is worse
/// than storing it once (`tenants.created_unix`).
fn parse_created_at_unix(created_at: &str) -> Option<i64> {
    created_at
        .strip_prefix("unix:")?
        .split('.')
        .next()?
        .parse()
        .ok()
}

/// PRD-mcphost-tenant-attribution requirement 5 / AC3: reclassify every
/// `tenants` row with `source_class IS NULL`, in place -- `id`/`class`/
/// `synthetic`/`created_unix` follow exactly requirement 1's rule
/// (`state::is_known_fleet_display_name`) and requirement 5's backfill
/// rule ("all loopback ⇒ synthetic", `classified_by = 'backfill'`). Every
/// signup before this PRD shipped came from 127.0.0.1 (verified
/// 2026-09-08: `signup_events.source_ip` is 127.0.0.1 for all 139 rows),
/// and `tenants` never stored a row's source IP -- so a pre-existing row's
/// class is derived from `display_name` alone, never a guess at an IP
/// this table doesn't have. Returns the number of rows reclassified.
///
/// Extracted out of [`Db::migrate_0010_tenant_attribution`] so the
/// migration's one-shot gate and this function's own logic are two
/// separable concerns: the migration decides *when* to backfill (once,
/// right after the columns land), this function decides *how*. Also lets
/// an integration test exercise the backfill rule directly against
/// `source_class IS NULL` fixture rows, without needing a genuine
/// pre-0010 database file (`Db::open` always runs every migration, so a
/// test can't otherwise catch this crate between 0009 and 0010).
fn backfill_unclassified_tenants_sync(conn: &Connection) -> Result<i64, AppError> {
    let rows: Vec<(i64, String, String, Option<String>)> = {
        let mut stmt = conn.prepare(
            "SELECT id, display_name, created_at, synthetic FROM tenants \
             WHERE source_class IS NULL",
        )?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let count = rows.len() as i64;
    for (id, display_name, created_at, existing_synthetic) in rows {
        let class = if crate::state::is_known_fleet_display_name(&display_name) {
            "fleet"
        } else {
            "loopback"
        };
        // Requirement 1: an explicit stamp this tenant already carried
        // always wins over the backfill default.
        let synthetic = existing_synthetic.unwrap_or_else(|| "harness:unstamped".to_string());
        let created_unix = parse_created_at_unix(&created_at);
        conn.execute(
            "UPDATE tenants SET source_class = ?1, synthetic = ?2, classified_by = 'backfill', \
             created_unix = ?3 WHERE id = ?4",
            params![class, synthetic, created_unix, id],
        )?;
    }
    Ok(count)
}

/// PRD-mcphost-provenance-audit requirement 1 / AC3: migration 0012's
/// one-shot backfill for every row still `origin = 'unclassified'`.
/// Counts what's about to be reclassified per bucket BEFORE each table's
/// own `UPDATE` runs (counting after the `UPDATE` can't distinguish
/// "touched by this pass" from "already was this value", since both read
/// the same final value), then applies the bucketing rule in order
/// tenants -> signup_events -> calls -- `calls`' bucket count (and its
/// `UPDATE`) both run after `tenants` has already been updated, so its
/// subselect against `tenants.origin` reads post-backfill tenant origins,
/// not the pre-backfill `'unclassified'` placeholder. Also the logic
/// behind the directly-testable [`Db::backfill_provenance`] -- same split
/// as `backfill_unclassified_tenants_sync`/`Db::backfill_unclassified_tenants`
/// above.
fn backfill_provenance_sync(conn: &Connection) -> Result<ProvenanceBackfillCounts, AppError> {
    let mut counts = ProvenanceBackfillCounts::default();

    let tenant_buckets: Vec<(String, i64)> = {
        let mut stmt = conn.prepare(
            "SELECT CASE WHEN source_class IN ('loopback','fleet') OR synthetic IS NOT NULL \
             THEN 'synthetic' ELSE 'external' END AS bucket, COUNT(*) FROM tenants \
             WHERE origin = 'unclassified' GROUP BY bucket",
        )?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (bucket, n) in tenant_buckets {
        match bucket.as_str() {
            "synthetic" => counts.tenants_synthetic = n,
            _ => counts.tenants_external = n,
        }
    }

    let signup_buckets: Vec<(String, i64)> = {
        let mut stmt = conn.prepare(
            "SELECT CASE WHEN synthetic IS NOT NULL OR source_ip IN ('127.0.0.1','::1') \
             THEN 'synthetic' ELSE 'external' END AS bucket, COUNT(*) FROM signup_events \
             WHERE origin = 'unclassified' GROUP BY bucket",
        )?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (bucket, n) in signup_buckets {
        match bucket.as_str() {
            "synthetic" => counts.signup_events_synthetic = n,
            _ => counts.signup_events_external = n,
        }
    }

    conn.execute(
        "UPDATE tenants SET origin = CASE WHEN source_class IN ('loopback','fleet') OR synthetic IS NOT NULL THEN 'synthetic' ELSE 'external' END, \
         origin_detail = CASE WHEN synthetic IS NOT NULL THEN synthetic WHEN source_class IN ('loopback','fleet') THEN source_class ELSE 'external-unverified' END \
         WHERE origin = 'unclassified'",
        [],
    )?;
    conn.execute(
        "UPDATE signup_events SET origin = CASE WHEN synthetic IS NOT NULL OR source_ip IN ('127.0.0.1','::1') THEN 'synthetic' ELSE 'external' END, \
         origin_detail = CASE WHEN synthetic IS NOT NULL THEN synthetic WHEN source_ip IN ('127.0.0.1','::1') THEN 'loopback' ELSE 'external-unverified' END \
         WHERE origin = 'unclassified'",
        [],
    )?;

    // Run AFTER the tenants UPDATE above, per the doc comment: this
    // subselect must see post-backfill tenant origins.
    let call_buckets: Vec<(String, i64)> = {
        let mut stmt = conn.prepare(
            "SELECT CASE WHEN (SELECT origin FROM tenants WHERE tenants.id = calls.tenant_id) = 'synthetic' \
             THEN 'synthetic' ELSE 'external' END AS bucket, COUNT(*) FROM calls \
             WHERE origin = 'unclassified' GROUP BY bucket",
        )?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (bucket, n) in call_buckets {
        match bucket.as_str() {
            "synthetic" => counts.calls_synthetic = n,
            _ => counts.calls_external = n,
        }
    }

    conn.execute(
        "UPDATE calls SET origin = COALESCE((SELECT origin FROM tenants WHERE tenants.id = calls.tenant_id), 'external'), \
         origin_detail = (SELECT origin_detail FROM tenants WHERE tenants.id = calls.tenant_id) \
         WHERE calls.origin = 'unclassified'",
        [],
    )?;

    Ok(counts)
}

/// Stamp `tenants.last_tool_change_unix` to now for `tenant_id`. Called by
/// both `upsert_tool` and `remove_tool` (AC18): the ttlMs cache hint in
/// `tools/list` must go to 0 after either, and only a tenant-level stamp
/// survives a remove deleting the tools row that carried its own timestamp.
fn touch_tenant_tool_change(conn: &Connection, tenant_id: i64) -> Result<(), AppError> {
    conn.execute(
        "UPDATE tenants SET last_tool_change_unix = ?1 WHERE id = ?2",
        params![now_unix(), tenant_id],
    )?;
    Ok(())
}

fn percentile(sorted: &[i64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = (p * (sorted.len() as f64 - 1.0)).round() as usize;
    sorted[rank.min(sorted.len() - 1)] as f64
}

pub struct Db {
    conn: Arc<Mutex<Connection>>,
    #[allow(dead_code)]
    path: PathBuf,
}

impl Clone for Db {
    fn clone(&self) -> Self {
        Self {
            conn: self.conn.clone(),
            path: self.path.clone(),
        }
    }
}

impl Db {
    /// Open (creating if absent) `<data_dir>/mcphost.db` in WAL mode and run
    /// migrations.
    pub fn open(data_dir: &Path) -> Result<Self, AppError> {
        std::fs::create_dir_all(data_dir)
            .map_err(|e| AppError::Storage(format!("cannot create data dir: {e}")))?;
        let path = data_dir.join("mcphost.db");
        let conn = Connection::open(&path).map_err(AppError::from)?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(AppError::from)?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(AppError::from)?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(AppError::from)?;
        let db = Db {
            conn: Arc::new(Mutex::new(conn)),
            path,
        };
        db.migrate_sync()?;
        Ok(db)
    }

    fn migrate_sync(&self) -> Result<(), AppError> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| AppError::Storage("db lock poisoned".into()))?;
        conn.execute_batch(MIGRATION_0001).map_err(AppError::from)?;
        Self::migrate_0002_tenant_last_tool_change(&conn)?;
        Self::migrate_0003_registry(&conn)?;
        Self::migrate_0004_calls_resource_usage(&conn)?;
        Self::migrate_0005_cascade_delete(&conn)?;
        Self::migrate_0006_billing(&conn)?;
        Self::migrate_0007_metering(&conn)?;
        Self::migrate_0008_synthetic(&conn)?;
        Self::migrate_0009_call_outcome(&conn)?;
        Self::migrate_0010_tenant_attribution(&conn)?;
        Self::migrate_0011_tenant_state(&conn)?;
        Self::migrate_0012_provenance(&conn)?;
        Self::migrate_0013_sharing(&conn)?;
        Self::migrate_0014_runs(&conn)?;
        Self::migrate_0015_triggers(&conn)?;
        Self::migrate_0016_runs_manual(&conn)
    }

    /// 0002 is a single `ALTER TABLE ADD COLUMN`, which SQLite has no
    /// `IF NOT EXISTS` guard for, so we check `pragma_table_info` first to
    /// keep `migrate()` idempotent (it runs at every `serve` start, per the
    /// doc comment on `migrate()` below).
    fn migrate_0002_tenant_last_tool_change(conn: &Connection) -> Result<(), AppError> {
        let has_column: bool = conn
            .prepare(
                "SELECT 1 FROM pragma_table_info('tenants') WHERE name = 'last_tool_change_unix'",
            )?
            .exists([])?;
        if !has_column {
            conn.execute_batch(MIGRATION_0002)?;
        }
        Ok(())
    }

    /// Same idempotency pattern as 0002: `pragma_table_info` gates the
    /// whole 0003 batch (two `ALTER TABLE`s plus the `registry_documents`
    /// table) on whether `namespace_verified` has already been added.
    fn migrate_0003_registry(conn: &Connection) -> Result<(), AppError> {
        let has_column: bool = conn
            .prepare(
                "SELECT 1 FROM pragma_table_info('tenants') WHERE name = 'namespace_verified'",
            )?
            .exists([])?;
        if !has_column {
            conn.execute_batch(MIGRATION_0003)?;
        }
        Ok(())
    }

    /// PRD-mcphost-code-tools requirement 8 / migration 0004: same
    /// idempotency pattern as 0002/0003, gated on `calls.cpu_ms`.
    fn migrate_0004_calls_resource_usage(conn: &Connection) -> Result<(), AppError> {
        let has_column: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('calls') WHERE name = 'cpu_ms'")?
            .exists([])?;
        if !has_column {
            conn.execute_batch(MIGRATION_0004)?;
        }
        Ok(())
    }

    /// PRD-mcphost-tenant-delete requirement 3 / AC6: gated on whether
    /// `tools`' foreign key to `tenants` already carries `ON DELETE
    /// CASCADE` -- unlike 0002-0004's `ADD COLUMN`, this migration
    /// recreates tables, so it needs its own idempotency signal rather
    /// than a column check.
    fn migrate_0005_cascade_delete(conn: &Connection) -> Result<(), AppError> {
        let has_cascade: bool = conn
            .prepare(
                "SELECT 1 FROM pragma_foreign_key_list('tools') \
                 WHERE \"table\" = 'tenants' AND on_delete = 'CASCADE'",
            )?
            .exists([])?;
        if !has_cascade {
            conn.execute_batch(MIGRATION_0005)?;
        }
        Ok(())
    }

    /// PRD-grand-loop-billing migration 0006: same idempotency pattern as
    /// 0002-0004, gated on `tenants.plan`.
    fn migrate_0006_billing(conn: &Connection) -> Result<(), AppError> {
        let has_column: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('tenants') WHERE name = 'plan'")?
            .exists([])?;
        if !has_column {
            conn.execute_batch(MIGRATION_0006)?;
        }
        Ok(())
    }

    /// PRD-mcphost-metered-overage migration 0007: same idempotency pattern
    /// as 0002-0006, gated on `tenants.stripe_customer_id`.
    fn migrate_0007_metering(conn: &Connection) -> Result<(), AppError> {
        let has_column: bool = conn
            .prepare(
                "SELECT 1 FROM pragma_table_info('tenants') WHERE name = 'stripe_customer_id'",
            )?
            .exists([])?;
        if !has_column {
            conn.execute_batch(MIGRATION_0007)?;
        }
        Ok(())
    }

    /// PRD-mcphost-synthetic-flag migration 0008: same idempotency pattern
    /// as 0002-0007, gated on `tenants.synthetic` (the batch also adds
    /// `signup_events.synthetic`, which has no independent gate -- both
    /// columns land together, atomically, the first time this runs).
    fn migrate_0008_synthetic(conn: &Connection) -> Result<(), AppError> {
        let has_column: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('tenants') WHERE name = 'synthetic'")?
            .exists([])?;
        if !has_column {
            conn.execute_batch(MIGRATION_0008)?;
        }
        Ok(())
    }

    /// PRD-mcphost-first-call-reliability migration 0009 (requirement 6):
    /// same idempotency pattern as 0002-0008, gated on `calls.outcome`.
    fn migrate_0009_call_outcome(conn: &Connection) -> Result<(), AppError> {
        let has_column: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('calls') WHERE name = 'outcome'")?
            .exists([])?;
        if !has_column {
            conn.execute_batch(MIGRATION_0009)?;
        }
        Ok(())
    }

    /// PRD-mcphost-tenant-attribution migration 0010 (requirements 1, 2,
    /// 5): same idempotency pattern as 0002-0009, gated on
    /// `tenants.source_class`. The `ALTER TABLE`s land first, then --
    /// still inside this one gate, so it only ever runs once -- every
    /// pre-existing tenant (the rows that are `source_class IS NULL`
    /// immediately after the `ALTER TABLE`) is reclassified in place
    /// (requirement 5 / AC3, "backfill" section of the migration file
    /// doc-comment).
    fn migrate_0010_tenant_attribution(conn: &Connection) -> Result<(), AppError> {
        let has_column: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('tenants') WHERE name = 'source_class'")?
            .exists([])?;
        if has_column {
            return Ok(());
        }
        conn.execute_batch(MIGRATION_0010)?;
        backfill_unclassified_tenants_sync(conn)?;
        Ok(())
    }

    /// PRD-mcphost-tenant-state migration 0011: all three tables are
    /// `CREATE TABLE IF NOT EXISTS`, so this only needs to gate on the
    /// batch's own idempotency signal (the first of the three) to avoid
    /// re-running an already-applied `CREATE INDEX` needlessly on every
    /// `serve` start, same pattern as 0002-0010.
    fn migrate_0011_tenant_state(conn: &Connection) -> Result<(), AppError> {
        let has_table: bool = conn
            .prepare(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'tenant_state_kv'",
            )?
            .exists([])?;
        if !has_table {
            conn.execute_batch(MIGRATION_0011)?;
        }
        Ok(())
    }

    /// PRD-mcphost-provenance-audit migration 0012 (requirements 1, 4): same
    /// idempotency pattern as 0002-0011, gated on `tenants.origin`. The
    /// `ALTER TABLE`s/`CREATE TABLE` land first, then -- still inside this
    /// one gate -- every pre-existing row (`origin = 'unclassified'`
    /// immediately after the `ALTER TABLE`s) is reclassified in place by
    /// [`backfill_provenance_sync`], with the per-table/per-bucket counts
    /// journaled (requirement 1, "counts journaled").
    fn migrate_0012_provenance(conn: &Connection) -> Result<(), AppError> {
        let has_column: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('tenants') WHERE name = 'origin'")?
            .exists([])?;
        if has_column {
            return Ok(());
        }
        conn.execute_batch(MIGRATION_0012)?;
        let counts = backfill_provenance_sync(conn)?;
        tracing::info!(?counts, "provenance backfill complete");
        Ok(())
    }

    /// PRD-mcphost-sharing migration 0013 (P0 requirement 1): same
    /// idempotency pattern as 0002-0012, gated on `tools.visibility`. Every
    /// existing tool becomes `'private'` via the column's own `DEFAULT`, so
    /// no backfill pass is needed (unlike 0010/0012, which reclassify
    /// pre-existing rows).
    fn migrate_0013_sharing(conn: &Connection) -> Result<(), AppError> {
        let has_column: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('tools') WHERE name = 'visibility'")?
            .exists([])?;
        if !has_column {
            conn.execute_batch(MIGRATION_0013)?;
        }
        Ok(())
    }

    /// PRD-mcphost-runs-and-jobs migration 0014 (P0 requirement 1): same
    /// idempotency pattern as 0011 (a wholly new, `CREATE TABLE IF NOT
    /// EXISTS`-guarded table), gated on the table's own existence.
    fn migrate_0014_runs(conn: &Connection) -> Result<(), AppError> {
        let has_table: bool = conn
            .prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'runs'")?
            .exists([])?;
        if !has_table {
            conn.execute_batch(MIGRATION_0014)?;
        }
        Ok(())
    }

    /// PRD-mcphost-schedules P0 requirement 1: same new-table idempotency
    /// guard as 0011/0014 above.
    fn migrate_0015_triggers(conn: &Connection) -> Result<(), AppError> {
        let has_table: bool = conn
            .prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'triggers'")?
            .exists([])?;
        if !has_table {
            conn.execute_batch(MIGRATION_0015)?;
        }
        Ok(())
    }

    /// PRD-mcphost-schedules P1 requirement 7: `ALTER TABLE ADD COLUMN` has
    /// no `IF NOT EXISTS`, so this checks `pragma_table_info` first, same
    /// idempotency guard 0002/0013 above use for their own added columns.
    fn migrate_0016_runs_manual(conn: &Connection) -> Result<(), AppError> {
        let has_column: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('runs') WHERE name = 'manual'")?
            .exists([])?;
        if !has_column {
            conn.execute_batch(MIGRATION_0016)?;
        }
        Ok(())
    }

    /// Run pending migrations. Idempotent (every statement is `IF NOT
    /// EXISTS`); used by both `serve` at start and the standalone `migrate`
    /// subcommand.
    pub async fn migrate(&self) -> Result<(), AppError> {
        let this = self.clone();
        tokio::task::spawn_blocking(move || this.migrate_sync())
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?
    }

    async fn with_conn<F, T>(&self, f: F) -> Result<T, AppError>
    where
        F: FnOnce(&Connection) -> Result<T, AppError> + Send + 'static,
        T: Send + 'static,
    {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let guard = conn
                .lock()
                .map_err(|_| AppError::Storage("db lock poisoned".into()))?;
            f(&guard)
        })
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
    }

    // ---- health ----------------------------------------------------

    /// A real writability probe: `BEGIN IMMEDIATE` acquires a write lock and
    /// fails with `SQLITE_READONLY` if the database cannot be written to
    /// (a genuinely read-only file, or `PRAGMA query_only = ON`, which is
    /// what the AC14 integration test uses to simulate an unwritable
    /// database without needing filesystem-permission tricks that a stray
    /// open fd would ignore).
    pub async fn is_writable(&self) -> bool {
        self.with_conn(|conn| {
            let result = conn.execute_batch("BEGIN IMMEDIATE; ROLLBACK;");
            Ok(result.is_ok())
        })
        .await
        .unwrap_or(false)
    }

    /// Test/ops-only: toggle `PRAGMA query_only` to simulate an unwritable
    /// database (AC14) without relying on filesystem permission semantics
    /// that don't apply to already-open file descriptors.
    pub async fn set_query_only(&self, on: bool) -> Result<(), AppError> {
        let value = if on { "ON" } else { "OFF" };
        let value = value.to_string();
        self.with_conn(move |conn| {
            conn.pragma_update(None, "query_only", value)
                .map_err(AppError::from)
        })
        .await
    }

    pub async fn counts(&self) -> Result<(i64, i64), AppError> {
        self.with_conn(|conn| {
            let tools: i64 = conn.query_row("SELECT COUNT(*) FROM tools", [], |r| r.get(0))?;
            let tenants: i64 = conn.query_row("SELECT COUNT(*) FROM tenants", [], |r| r.get(0))?;
            Ok((tools, tenants))
        })
        .await
    }

    /// PRD-mcphost-tenant-delete requirement 5 / AC9: tenants whose
    /// `display_name` starts with `panel_` or `probe-` -- the two prefixes
    /// the harness's synthetic personas use (see `admin.tenant_delete_by_prefix`
    /// and the PRD's user stories). Both prefixes are 6 bytes/characters,
    /// so a plain `substr` comparison avoids needing to escape `LIKE`
    /// wildcards a prefix might itself contain.
    pub async fn probe_tenant_count(&self) -> Result<i64, AppError> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM tenants \
                 WHERE substr(display_name, 1, 6) = 'panel_' \
                    OR substr(display_name, 1, 6) = 'probe-'",
                [],
                |r| r.get(0),
            )
            .map_err(AppError::from)
        })
        .await
    }

    // ---- tenants -----------------------------------------------------

    pub async fn create_tenant(
        &self,
        display_name: String,
        namespace: String,
        key_hash: String,
        synthetic: Option<String>,
    ) -> Result<Tenant, AppError> {
        // PRD-mcphost-provenance-audit: this wrapper's signature stays
        // unchanged (existing test-only callers must keep compiling
        // unmodified) -- `origin`/`origin_detail` are derived from the
        // same `synthetic` label every other field here has always used.
        let origin = if synthetic.is_some() {
            "synthetic".to_string()
        } else {
            "external".to_string()
        };
        let origin_detail = synthetic.clone();
        self.create_tenant_attributed(
            display_name,
            namespace,
            key_hash,
            synthetic,
            None,
            None,
            None,
            origin,
            origin_detail,
        )
        .await
    }

    /// PRD-mcphost-tenant-attribution requirements 1-2: [`Self::create_tenant`]
    /// plus the derived `source_class` and captured `clientInfo`, all set
    /// atomically with the row's insert rather than in a follow-up
    /// `UPDATE` -- so a tenant is never, even briefly, in the
    /// unclassified state migration 0010's backfill exists to clean up.
    /// `classified_by` is always `None` here: only the migration's
    /// one-shot backfill ever writes `"backfill"` (requirement 5 / AC3).
    /// `synthetic` is the caller's already-fully-derived value (an
    /// explicit stamp, or `Some("harness:unstamped")`, or `None` for
    /// `external` -- see `control::signup`), stored as given, same as
    /// [`Self::create_tenant`] always has.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_tenant_attributed(
        &self,
        display_name: String,
        namespace: String,
        key_hash: String,
        synthetic: Option<String>,
        source_class: Option<String>,
        client_name: Option<String>,
        client_version: Option<String>,
        origin: String,
        origin_detail: Option<String>,
    ) -> Result<Tenant, AppError> {
        let created_at = now_rfc3339();
        let created_unix = now_unix();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO tenants (namespace, display_name, key_hash, created_at, disabled, \
                 synthetic, source_class, client_name, client_version, created_unix, origin, origin_detail) \
                 VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    namespace,
                    display_name,
                    key_hash,
                    created_at,
                    synthetic,
                    source_class,
                    client_name,
                    client_version,
                    created_unix,
                    origin,
                    origin_detail,
                ],
            )?;
            let id = conn.last_insert_rowid();
            Ok(Tenant {
                id,
                namespace,
                display_name,
                key_hash,
                created_at,
                disabled: false,
                last_tool_change_unix: 0,
                namespace_verified: false,
                registry_namespace: None,
                plan: "free".to_string(),
                plan_since: None,
                billing_ref: None,
                stripe_customer_id: None,
                synthetic,
                source_class,
                client_name,
                client_version,
                classified_by: None,
                created_unix: Some(created_unix),
                origin,
                origin_detail,
            })
        })
        .await
    }

    /// PRD-mcphost-tenant-attribution requirement 5 / AC3: the public,
    /// directly-testable entry point for [`backfill_unclassified_tenants_sync`]
    /// -- also what `migrate_0010_tenant_attribution` calls once, right
    /// after the migration's `ALTER TABLE`s land. Returns the number of
    /// rows reclassified (0 once nothing is left `source_class IS NULL`).
    pub async fn backfill_unclassified_tenants(&self) -> Result<i64, AppError> {
        self.with_conn(backfill_unclassified_tenants_sync).await
    }

    /// PRD-mcphost-provenance-audit requirement 1 / AC3: the public,
    /// directly-testable entry point for [`backfill_provenance_sync`] --
    /// also what `migrate_0012_provenance` calls once, right after
    /// migration 0012's `ALTER TABLE`s land. Returns all-zero counts once
    /// nothing is left `origin = 'unclassified'` anywhere.
    pub async fn backfill_provenance(&self) -> Result<ProvenanceBackfillCounts, AppError> {
        self.with_conn(backfill_provenance_sync).await
    }

    /// PRD-mcphost-tenant-attribution requirement 2's "first authenticated
    /// session if signup preceded capture" fallback: sets `client_name`/
    /// `client_version` only if the row still has neither (the `WHERE
    /// client_name IS NULL` guard), so a later session's `clientInfo`
    /// never overwrites the one captured at signup.
    pub async fn set_tenant_client_info(
        &self,
        tenant_id: i64,
        client_name: String,
        client_version: String,
    ) -> Result<(), AppError> {
        self.with_conn(move |conn| {
            conn.execute(
                "UPDATE tenants SET client_name = ?1, client_version = ?2 \
                 WHERE id = ?3 AND client_name IS NULL",
                params![client_name, client_version, tenant_id],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn find_tenant_by_key_hash(
        &self,
        key_hash: String,
    ) -> Result<Option<Tenant>, AppError> {
        self.with_conn(move |conn| {
            let sql = format!("SELECT {TENANT_COLUMNS} FROM tenants WHERE key_hash = ?1");
            conn.query_row(&sql, params![key_hash], tenant_from_row)
                .optional()
                .map_err(AppError::from)
        })
        .await
    }

    pub async fn find_tenant_by_namespace(
        &self,
        namespace: String,
    ) -> Result<Option<Tenant>, AppError> {
        self.with_conn(move |conn| {
            let sql = format!("SELECT {TENANT_COLUMNS} FROM tenants WHERE namespace = ?1");
            conn.query_row(&sql, params![namespace], tenant_from_row)
                .optional()
                .map_err(AppError::from)
        })
        .await
    }

    /// PRD-mcphost-runs-and-jobs: the executor only ever has a leased
    /// `runs.tenant_id` to work from (no namespace, no bearer key on hand),
    /// so it needs a by-id lookup the other tenant resolvers above don't
    /// provide.
    pub async fn find_tenant_by_id(&self, tenant_id: i64) -> Result<Option<Tenant>, AppError> {
        self.with_conn(move |conn| {
            let sql = format!("SELECT {TENANT_COLUMNS} FROM tenants WHERE id = ?1");
            conn.query_row(&sql, params![tenant_id], tenant_from_row)
                .optional()
                .map_err(AppError::from)
        })
        .await
    }

    /// PRD-grand-loop-billing: resolve a tenant by the payment processor's
    /// own id (`billing_ref` -- a Stripe customer or subscription id),
    /// used by webhook events (`invoice.paid`,
    /// `customer.subscription.deleted`, `invoice.payment_failed`) that
    /// carry no `client_reference_id` of their own.
    pub async fn find_tenant_by_billing_ref(
        &self,
        billing_ref: String,
    ) -> Result<Option<Tenant>, AppError> {
        self.with_conn(move |conn| {
            let sql = format!("SELECT {TENANT_COLUMNS} FROM tenants WHERE billing_ref = ?1");
            conn.query_row(&sql, params![billing_ref], tenant_from_row)
                .optional()
                .map_err(AppError::from)
        })
        .await
    }

    pub async fn set_tenant_disabled(
        &self,
        namespace: String,
        disabled: bool,
    ) -> Result<bool, AppError> {
        self.with_conn(move |conn| {
            let n = conn.execute(
                "UPDATE tenants SET disabled = ?1 WHERE namespace = ?2",
                params![disabled as i64, namespace],
            )?;
            Ok(n > 0)
        })
        .await
    }

    /// PRD-mcphost-synthetic-flag AC5: `admin.tenant_set_synthetic` sets or
    /// clears (`label: None`) one tenant's `synthetic` label by namespace,
    /// same identifier every other single-tenant admin tool in this file
    /// uses (`tenant_disable`, `tenant_enable`, `tenant_verify_namespace`).
    pub async fn set_tenant_synthetic(
        &self,
        namespace: String,
        label: Option<String>,
    ) -> Result<bool, AppError> {
        self.with_conn(move |conn| {
            let n = conn.execute(
                "UPDATE tenants SET synthetic = ?1 WHERE namespace = ?2",
                params![label, namespace],
            )?;
            Ok(n > 0)
        })
        .await
    }

    /// AC19: `admin.tenant_verify_namespace` sets the per-tenant
    /// "domain namespace verified" boolean plus the reverse-DNS-style
    /// namespace it was verified under. The verification METHOD is not
    /// this crate's concern (PRD Open Questions) -- this is just storage
    /// for the admin's say-so.
    pub async fn set_tenant_namespace_verified(
        &self,
        namespace: String,
        registry_namespace: String,
    ) -> Result<bool, AppError> {
        self.with_conn(move |conn| {
            let n = conn.execute(
                "UPDATE tenants SET namespace_verified = 1, registry_namespace = ?1 WHERE namespace = ?2",
                params![registry_namespace, namespace],
            )?;
            Ok(n > 0)
        })
        .await
    }

    pub async fn list_tenants(&self) -> Result<Vec<Tenant>, AppError> {
        self.with_conn(|conn| {
            let sql = format!("SELECT {TENANT_COLUMNS} FROM tenants ORDER BY id");
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map([], tenant_from_row)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// PRD-grand-loop-billing AC11: tenants whose `plan` is not `free`.
    pub async fn count_paying_tenants(&self) -> Result<i64, AppError> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM tenants WHERE plan != 'free'",
                [],
                |r| r.get(0),
            )
            .map_err(AppError::from)
        })
        .await
    }

    /// PRD-mcphost-synthetic-flag AC7: labeled-tenant count for
    /// `/healthz`'s `tenants_synthetic` (`tenants_real` is `tenants_total`
    /// minus this, computed by the caller since `counts()` already reads
    /// `tenants_total`).
    pub async fn count_synthetic_tenants(&self) -> Result<i64, AppError> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM tenants WHERE synthetic IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .map_err(AppError::from)
        })
        .await
    }

    /// PRD-mcphost-tenant-attribution requirement 3 / AC1/AC3/AC4:
    /// `tenants_real`'s new definition -- `source_class = 'external'`,
    /// not "has no `synthetic` label" (the old, buggy definition
    /// [`Self::count_synthetic_tenants`] backed -- every loopback/fleet
    /// signup now carries a `synthetic` label too, per migration 0010, so
    /// that column alone can no longer answer "is this tenant real").
    pub async fn count_external_tenants(&self) -> Result<i64, AppError> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM tenants WHERE source_class = 'external'",
                [],
                |r| r.get(0),
            )
            .map_err(AppError::from)
        })
        .await
    }

    /// PRD-mcphost-provenance-audit requirement 3: `/healthz`'s
    /// `tenants.{external,synthetic}` split, keyed off migration 0012's
    /// `origin` column (not `source_class`, which [`Self::count_external_tenants`]
    /// still uses for its own, narrower, pre-existing purpose). Returns
    /// `(external, synthetic)`.
    pub async fn count_tenants_by_origin(&self) -> Result<(i64, i64), AppError> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT (SELECT COUNT(*) FROM tenants WHERE origin = 'external'), \
                        (SELECT COUNT(*) FROM tenants WHERE origin = 'synthetic')",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(AppError::from)
        })
        .await
    }

    /// PRD-mcphost-provenance-audit requirement 3: `/healthz`'s
    /// `signups.{external,synthetic}` split. Returns `(external, synthetic)`.
    pub async fn count_signup_events_by_origin(&self) -> Result<(i64, i64), AppError> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT (SELECT COUNT(*) FROM signup_events WHERE origin = 'external'), \
                        (SELECT COUNT(*) FROM signup_events WHERE origin = 'synthetic')",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(AppError::from)
        })
        .await
    }

    /// PRD-mcphost-provenance-audit requirement 3: `/healthz`'s
    /// `calls.{external,synthetic}` split. Returns `(external, synthetic)`.
    pub async fn count_calls_by_origin(&self) -> Result<(i64, i64), AppError> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT (SELECT COUNT(*) FROM calls WHERE origin = 'external'), \
                        (SELECT COUNT(*) FROM calls WHERE origin = 'synthetic')",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(AppError::from)
        })
        .await
    }

    /// PRD-mcphost-tenant-attribution requirement 3: `tenants_by_source_class`
    /// -- every distinct `source_class` value present, counted, most
    /// common first. A `NULL` (a tenant somehow missed by both live
    /// classification and the migration 0010 backfill -- should never
    /// happen after `migrate()` runs) groups under `"unclassified"` rather
    /// than silently vanishing from the total.
    pub async fn count_tenants_by_source_class(&self) -> Result<Vec<(String, i64)>, AppError> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT COALESCE(source_class, 'unclassified'), COUNT(*) FROM tenants \
                 GROUP BY 1 ORDER BY 2 DESC, 1 ASC",
            )?;
            let rows = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await
    }

    /// PRD-mcphost-tenant-attribution requirement 3: `tenants_by_client`
    /// -- the top `limit` distinct `client_name` values by tenant count,
    /// most common first; ties break alphabetically so the result is
    /// deterministic. Tenants with no captured `client_name` yet are
    /// excluded, not grouped under a placeholder -- "which clients are our
    /// tenants using" shouldn't be diluted by "we don't know yet".
    pub async fn count_tenants_by_client(&self, limit: i64) -> Result<Vec<(String, i64)>, AppError> {
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT client_name, COUNT(*) FROM tenants WHERE client_name IS NOT NULL \
                 GROUP BY client_name ORDER BY COUNT(*) DESC, client_name ASC LIMIT ?1",
            )?;
            let rows = stmt
                .query_map(params![limit], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await
    }

    /// PRD-mcphost-tenant-attribution requirement 4 (`mcphost funnel`):
    /// every tenant's funnel-relevant fields, optionally restricted to
    /// tenants created at or after `since_unix`. Deliberately every column
    /// the funnel needs in one pass rather than reusing [`Self::list_tenants`]'s
    /// full `Tenant` -- this crate's small DB makes the difference moot
    /// today, but it keeps the funnel's own query independent of
    /// `TENANT_COLUMNS`' shape.
    pub async fn funnel_tenants(
        &self,
        since_unix: Option<i64>,
    ) -> Result<Vec<(i64, Option<String>, String, Option<i64>)>, AppError> {
        self.with_conn(move |conn| {
            let sql = match since_unix {
                Some(_) => {
                    "SELECT id, source_class, plan, created_unix FROM tenants \
                     WHERE created_unix IS NOT NULL AND created_unix >= ?1"
                }
                None => "SELECT id, source_class, plan, created_unix FROM tenants",
            };
            let mut stmt = conn.prepare(sql)?;
            let rows = match since_unix {
                Some(since) => stmt
                    .query_map(params![since], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?,
                None => stmt
                    .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?,
            };
            Ok(rows)
        })
        .await
    }

    /// The distinct set of tenant ids with at least one published tool
    /// (funnel stage "published") -- `mcphost funnel` needs the whole set
    /// once, not a per-tenant existence check.
    pub async fn published_tenant_ids(&self) -> Result<std::collections::HashSet<i64>, AppError> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare("SELECT DISTINCT tenant_id FROM tools")?;
            let ids = stmt
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<std::collections::HashSet<i64>>>()?;
            Ok(ids)
        })
        .await
    }

    /// Every call's `(tenant_id, started_unix, ok)`, oldest first --
    /// `mcphost funnel`'s only read of `calls`, used to derive "first
    /// successful call", "returned on a later day", and "hit the daily
    /// cap" (a day whose ok-call count reaches the tenant's plan quota;
    /// see `funnel::compute`) without a second table this PRD's technical
    /// considerations rule out.
    pub async fn funnel_calls(&self) -> Result<Vec<(i64, i64, bool)>, AppError> {
        self.with_conn(|conn| {
            let mut stmt =
                conn.prepare("SELECT tenant_id, started_unix, ok FROM calls ORDER BY tenant_id, started_unix")?;
            let rows = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await
    }

    /// PRD-mcphost-synthetic-flag AC10 / P1 requirement 10: `(real, synthetic)`
    /// paying-tenant counts in one query, so `/healthz` can add
    /// `paying_tenants_real` only when the synthetic half is nonzero
    /// (guards against a synthetic tenant ever polluting the revenue count
    /// while keeping the field absent -- not present-and-equal -- on a host
    /// where it can never have differed from `paying_tenants`).
    ///
    /// PRD-mcphost-tenant-attribution: "real" here now matches `/healthz`'s
    /// `tenants_real` (`source_class = 'external'`), not `synthetic IS
    /// NULL` -- migration 0010 means a loopback/fleet tenant always
    /// carries a `synthetic` label now, so the old predicate would read
    /// every paying tenant as "synthetic" the moment a real one existed
    /// alongside them, the same bug this PRD's TL;DR describes for
    /// `tenants_real` itself.
    pub async fn paying_tenant_synthetic_split(&self) -> Result<(i64, i64), AppError> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT \
                    SUM(CASE WHEN plan != 'free' AND source_class = 'external' THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN plan != 'free' AND source_class != 'external' THEN 1 ELSE 0 END) \
                 FROM tenants",
                [],
                |r| Ok((r.get::<_, Option<i64>>(0)?.unwrap_or(0), r.get::<_, Option<i64>>(1)?.unwrap_or(0))),
            )
            .map_err(AppError::from)
        })
        .await
    }

    /// AC6 / requirement "checkout.session.completed": set `plan`,
    /// `plan_since`, and `billing_ref` together -- also reused by
    /// `admin.plan_set`'s support override.
    pub async fn upgrade_tenant_plan(
        &self,
        tenant_id: i64,
        plan: String,
        plan_since: String,
        billing_ref: Option<String>,
    ) -> Result<(), AppError> {
        self.with_conn(move |conn| {
            conn.execute(
                "UPDATE tenants SET plan = ?1, plan_since = ?2, \
                 billing_ref = COALESCE(?3, billing_ref) WHERE id = ?4",
                params![plan, plan_since, billing_ref, tenant_id],
            )?;
            Ok(())
        })
        .await
    }

    /// AC8: `customer.subscription.deleted` / `invoice.payment_failed` --
    /// the tenant returns to `free` (Non-goals: "keeps its tools within
    /// the free quota", so nothing else about the tenant changes here).
    pub async fn downgrade_tenant_plan(&self, tenant_id: i64) -> Result<(), AppError> {
        self.with_conn(move |conn| {
            conn.execute(
                "UPDATE tenants SET plan = 'free' WHERE id = ?1",
                params![tenant_id],
            )?;
            Ok(())
        })
        .await
    }

    /// `invoice.paid`: refresh `plan_since` without touching `plan` or
    /// `billing_ref` (the subscription is simply renewing, not changing).
    pub async fn refresh_plan_since(
        &self,
        tenant_id: i64,
        plan_since: String,
    ) -> Result<(), AppError> {
        self.with_conn(move |conn| {
            conn.execute(
                "UPDATE tenants SET plan_since = ?1 WHERE id = ?2",
                params![plan_since, tenant_id],
            )?;
            Ok(())
        })
        .await
    }

    /// PRD-mcphost-tenant-delete requirement 2/7 (AC3/AC4/AC10):
    /// tenants whose `display_name` starts with `prefix`, ordered by id.
    /// `limit` is the batch-delete call's 500-tenant cap (requirement 6);
    /// `None` (used by `admin.tenants(prefix)`, which carries no such cap)
    /// returns every match. Returns `(matches, truncated)`, `truncated`
    /// true only when `limit` was given and more rows matched than it
    /// allowed.
    pub async fn list_tenants_by_prefix(
        &self,
        prefix: String,
        limit: Option<i64>,
    ) -> Result<(Vec<Tenant>, bool), AppError> {
        self.with_conn(move |conn| {
            let prefix_len = prefix.chars().count() as i64;
            let mut rows: Vec<Tenant> = if let Some(lim) = limit {
                let sql = format!(
                    "SELECT {TENANT_COLUMNS} FROM tenants \
                     WHERE substr(display_name, 1, ?1) = ?2 ORDER BY id LIMIT ?3"
                );
                let mut stmt = conn.prepare(&sql)?;
                stmt.query_map(params![prefix_len, prefix, lim + 1], tenant_from_row)?
                    .collect::<Result<Vec<_>, _>>()?
            } else {
                let sql = format!(
                    "SELECT {TENANT_COLUMNS} FROM tenants \
                     WHERE substr(display_name, 1, ?1) = ?2 ORDER BY id"
                );
                let mut stmt = conn.prepare(&sql)?;
                stmt.query_map(params![prefix_len, prefix], tenant_from_row)?
                    .collect::<Result<Vec<_>, _>>()?
            };
            let truncated = match limit {
                Some(lim) if rows.len() as i64 > lim => {
                    rows.truncate(lim as usize);
                    true
                }
                _ => false,
            };
            Ok((rows, truncated))
        })
        .await
    }

    /// PRD-mcphost-synthetic-flag AC6: `admin.tenants_set_synthetic`'s
    /// retro-tag -- `name_like` is a raw SQL `LIKE` pattern against
    /// `display_name` (the operator supplies its own `%`/`_` wildcards,
    /// e.g. `%Chen%`), unlike the prefix-only substr match
    /// `list_tenants_by_prefix` uses for `tenant_delete_by_prefix`, since
    /// this tool's job is finding panel personas by name shape rather than
    /// a fixed prefix. `dry_run` returns the matches unmodified; otherwise
    /// every match is updated to `label` (already validated by the caller)
    /// before being returned, so the response reflects the post-write state
    /// either way.
    pub async fn set_tenants_synthetic_by_name_like(
        &self,
        name_like: String,
        label: String,
        dry_run: bool,
    ) -> Result<Vec<Tenant>, AppError> {
        self.with_conn(move |conn| {
            let sql =
                format!("SELECT {TENANT_COLUMNS} FROM tenants WHERE display_name LIKE ?1 ORDER BY id");
            let mut stmt = conn.prepare(&sql)?;
            let mut matches: Vec<Tenant> = stmt
                .query_map(params![name_like], tenant_from_row)?
                .collect::<Result<Vec<_>, _>>()?;
            if !dry_run {
                for t in &mut matches {
                    conn.execute(
                        "UPDATE tenants SET synthetic = ?1 WHERE id = ?2",
                        params![label, t.id],
                    )?;
                    t.synthetic = Some(label.clone());
                }
            }
            Ok(matches)
        })
        .await
    }

    fn query_tenant_by_namespace(conn: &Connection, namespace: &str) -> Result<Option<Tenant>, AppError> {
        let sql = format!("SELECT {TENANT_COLUMNS} FROM tenants WHERE namespace = ?1");
        conn.query_row(&sql, params![namespace], tenant_from_row)
            .optional()
            .map_err(AppError::from)
    }

    /// PRD-mcphost-tenant-delete requirement 1/2: the row counts a delete
    /// of `tenant_id` would remove, without mutating anything -- used both
    /// by the batch call's dry run and internally by [`Db::delete_tenant`]
    /// itself (counted before the delete, since the rows won't exist to
    /// count afterward).
    fn count_tenant_child_rows(conn: &Connection, tenant_id: i64) -> Result<TenantDeleteCounts, AppError> {
        let tools_removed: i64 = conn.query_row(
            "SELECT COUNT(*) FROM tools WHERE tenant_id = ?1",
            params![tenant_id],
            |r| r.get(0),
        )?;
        let secrets_removed: i64 = conn.query_row(
            "SELECT COUNT(*) FROM secrets WHERE tenant_id = ?1",
            params![tenant_id],
            |r| r.get(0),
        )?;
        let calls_removed: i64 = conn.query_row(
            "SELECT COUNT(*) FROM calls WHERE tenant_id = ?1",
            params![tenant_id],
            |r| r.get(0),
        )?;
        let logs_removed: i64 = conn.query_row(
            "SELECT COUNT(*) FROM logs WHERE tenant_id = ?1",
            params![tenant_id],
            |r| r.get(0),
        )?;
        Ok(TenantDeleteCounts {
            tools_removed,
            secrets_removed,
            calls_removed,
            logs_removed,
        })
    }

    /// AC3's dry-run counts, by namespace: `None` if no such tenant.
    pub async fn tenant_delete_preview(
        &self,
        namespace: String,
    ) -> Result<Option<(Tenant, TenantDeleteCounts)>, AppError> {
        self.with_conn(move |conn| {
            let tenant = match Self::query_tenant_by_namespace(conn, &namespace)? {
                Some(t) => t,
                None => return Ok(None),
            };
            let counts = Self::count_tenant_child_rows(conn, tenant.id)?;
            Ok(Some((tenant, counts)))
        })
        .await
    }

    /// PRD-mcphost-tenant-delete requirement 1/3/4 (AC1/AC7/AC8): delete
    /// `namespace` and, in the same transaction, cascade every row that
    /// references it (migration 0005's `ON DELETE CASCADE` on tools,
    /// secrets, calls, logs, registry_documents), plus write one
    /// `admin_events` audit row. `None` if no such tenant exists --
    /// `admin.rs` turns that into `tenant_not_found`.
    ///
    /// Uses a hand-rolled `BEGIN IMMEDIATE` / `COMMIT` (rather than
    /// `Connection::transaction`, which needs `&mut Connection`) because
    /// this runs inside `with_conn`'s `&Connection` closure, behind the
    /// single shared connection's mutex.
    pub async fn delete_tenant(
        &self,
        namespace: String,
    ) -> Result<Option<(Tenant, TenantDeleteCounts)>, AppError> {
        self.with_conn(move |conn| {
            let tenant = match Self::query_tenant_by_namespace(conn, &namespace)? {
                Some(t) => t,
                None => return Ok(None),
            };
            let counts = Self::count_tenant_child_rows(conn, tenant.id)?;

            conn.execute("BEGIN IMMEDIATE", []).map_err(AppError::from)?;
            let outcome: Result<(), AppError> = (|| {
                conn.execute("DELETE FROM tenants WHERE id = ?1", params![tenant.id])?;
                let detail = json!({
                    "namespace": tenant.namespace,
                    "tools_removed": counts.tools_removed,
                    "secrets_removed": counts.secrets_removed,
                    "calls_removed": counts.calls_removed,
                    "logs_removed": counts.logs_removed,
                })
                .to_string();
                conn.execute(
                    "INSERT INTO admin_events (ts, action, tenant, detail) VALUES (?1, ?2, ?3, ?4)",
                    params![now_rfc3339(), "tenant_delete", tenant.namespace, detail],
                )?;
                Ok(())
            })();
            match outcome {
                Ok(()) => {
                    conn.execute("COMMIT", []).map_err(AppError::from)?;
                }
                Err(e) => {
                    let _ = conn.execute("ROLLBACK", []);
                    return Err(e);
                }
            }
            Ok(Some((tenant, counts)))
        })
        .await
    }

    /// PRD-mcphost-provenance-audit requirement 4: append a row to
    /// `admin_audit`, distinct from `admin_events` above (migration 0005,
    /// delete-only, no actor identity) -- `admin_audit` is this PRD's
    /// general-purpose, actor-tracked log every admin-bearer mutation
    /// appends to centrally in `handler::dispatch_admin_tool`, not just
    /// tenant deletion.
    pub async fn record_admin_audit(
        &self,
        actor_key_id: String,
        action: String,
        target: Option<String>,
        detail: Option<String>,
    ) -> Result<(), AppError> {
        let ts = now_unix();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO admin_audit (actor_key_id, action, target, detail, created_unix) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![actor_key_id, action, target, detail, ts],
            )?;
            Ok(())
        })
        .await
    }

    /// `admin.audit_log` (requirement 4): newest-first, paged read of
    /// `admin_audit`; `before_id` (when given) restricts to rows older
    /// than that id, so a caller can page backward through the log.
    pub async fn list_admin_audit(
        &self,
        limit: i64,
        before_id: Option<i64>,
    ) -> Result<Vec<AdminAuditRow>, AppError> {
        self.with_conn(move |conn| {
            let rows: Vec<AdminAuditRow> = match before_id {
                Some(before) => {
                    let mut stmt = conn.prepare(
                        "SELECT id, actor_key_id, action, target, detail, created_unix \
                         FROM admin_audit WHERE id < ?1 ORDER BY id DESC LIMIT ?2",
                    )?;
                    stmt.query_map(params![before, limit], admin_audit_row_from_row)?
                        .collect::<rusqlite::Result<Vec<_>>>()?
                }
                None => {
                    let mut stmt = conn.prepare(
                        "SELECT id, actor_key_id, action, target, detail, created_unix \
                         FROM admin_audit ORDER BY id DESC LIMIT ?1",
                    )?;
                    stmt.query_map(params![limit], admin_audit_row_from_row)?
                        .collect::<rusqlite::Result<Vec<_>>>()?
                }
            };
            Ok(rows)
        })
        .await
    }

    // ---- signup rate limiting -----------------------------------------

    pub async fn signup_count_since(
        &self,
        source_ip: String,
        since_unix: i64,
    ) -> Result<i64, AppError> {
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM signup_events WHERE source_ip = ?1 AND created_unix >= ?2",
                params![source_ip, since_unix],
                |r| r.get(0),
            )
            .map_err(AppError::from)
        })
        .await
    }

    /// PRD-mcphost-call-limits-honest requirement 4 (AC5): the check
    /// (`signup_count_since`'s own query) and the increment
    /// (`record_signup_event_attributed`'s own insert) as one statement,
    /// executed inside the single `with_conn` closure this `Db`'s every
    /// other method already funnels through -- since `conn` is
    /// `Arc<Mutex<Connection>>`, this holds the one lock for the whole
    /// check-and-insert, closing the race the old two-call sequence left
    /// open (a burst of concurrent `signup()` calls could all read
    /// `recent < limit` before any of them had written its own event).
    /// Returns `true` iff the insert actually happened (this signup is
    /// admitted); `false` means the cap was already at `limit` for this
    /// source/window and nothing was written -- the caller must not also
    /// call `record_signup_event_attributed` for this attempt.
    // Six parameters: this is the atomic check-and-insert described above —
    // splitting it into a params struct would spread one SQL statement's
    // inputs across two sites for a function with a single caller.
    #[allow(clippy::too_many_arguments)]
    pub async fn try_admit_signup(
        &self,
        source_ip: String,
        since_unix: i64,
        limit: i64,
        synthetic: Option<String>,
        user_agent: Option<String>,
        origin: String,
        origin_detail: Option<String>,
        ip_class: String,
    ) -> Result<bool, AppError> {
        let ts = now_unix();
        self.with_conn(move |conn| {
            let inserted = conn.execute(
                "INSERT INTO signup_events (source_ip, created_unix, synthetic, user_agent, origin, origin_detail, ip_class) \
                 SELECT ?1, ?2, ?3, ?4, ?7, ?8, ?9 \
                 WHERE (SELECT COUNT(*) FROM signup_events \
                        WHERE source_ip = ?1 AND created_unix >= ?5) < ?6",
                params![source_ip, ts, synthetic, user_agent, since_unix, limit, origin, origin_detail, ip_class],
            )?;
            Ok(inserted > 0)
        })
        .await
    }

    /// PRD-mcphost-client-ip-behind-proxy requirement 5 / AC6: `/healthz`'s
    /// `distinct_source_ips_24h` -- the count of distinct
    /// `signup_events.source_ip` values recorded in the last 24 hours, so
    /// the per-source-address fix (one shared bucket becoming per-caller)
    /// is visible from outside without a direct `sqlite3` query.
    pub async fn count_distinct_source_ips_since(&self, since_unix: i64) -> Result<i64, AppError> {
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT COUNT(DISTINCT source_ip) FROM signup_events WHERE created_unix >= ?1",
                params![since_unix],
                |r| r.get(0),
            )
            .map_err(AppError::from)
        })
        .await
    }

    /// `synthetic` (P2 requirement 8) is the same validated-or-null label
    /// `create_tenant` stores on the tenant row, recorded here too so the
    /// ledger of signup attempts is independently auditable even for a
    /// tenant later deleted or retro-tagged differently.
    pub async fn record_signup_event(
        &self,
        source_ip: String,
        synthetic: Option<String>,
    ) -> Result<(), AppError> {
        self.record_signup_event_attributed(source_ip, synthetic, None)
            .await
    }

    /// PRD-mcphost-tenant-attribution requirement 2: [`Self::record_signup_event`]
    /// plus the transport's `User-Agent`, when it exposes one -- the
    /// durable per-signup ledger this PRD's technical considerations call
    /// out separately from `tenants.synthetic`'s current-state column.
    pub async fn record_signup_event_attributed(
        &self,
        source_ip: String,
        synthetic: Option<String>,
        user_agent: Option<String>,
    ) -> Result<(), AppError> {
        let ts = now_unix();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO signup_events (source_ip, created_unix, synthetic, user_agent) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![source_ip, ts, synthetic, user_agent],
            )?;
            Ok(())
        })
        .await
    }

    // ---- tools ---------------------------------------------------------

    pub async fn count_tools(&self, tenant_id: i64) -> Result<i64, AppError> {
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM tools WHERE tenant_id = ?1",
                params![tenant_id],
                |r| r.get(0),
            )
            .map_err(AppError::from)
        })
        .await
    }

    pub async fn get_tool(
        &self,
        tenant_id: i64,
        name: String,
    ) -> Result<Option<ToolRow>, AppError> {
        self.with_conn(move |conn| {
            let sql = format!("SELECT {TOOL_COLUMNS} FROM tools WHERE tenant_id = ?1 AND name = ?2");
            conn.query_row(&sql, params![tenant_id, name], tool_from_row)
                .optional()
                .map_err(AppError::from)
        })
        .await
    }

    /// Insert or replace a tenant's tool. The caller (`control::tool_publish`)
    /// is responsible for the 50-tool limit check (this call must not itself
    /// be the thing enforcing it, since replacing an existing tool must not
    /// count against the limit).
    pub async fn upsert_tool(
        &self,
        tenant_id: i64,
        name: String,
        kind: String,
        spec: Value,
    ) -> Result<(), AppError> {
        let created_at = now_rfc3339();
        let spec_text = serde_json::to_string(&spec)
            .map_err(|e| AppError::Internal(format!("spec serialize: {e}")))?;
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO tools (tenant_id, name, kind, spec, created_at) VALUES (?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT(tenant_id, name) DO UPDATE SET kind = excluded.kind, spec = excluded.spec",
                params![tenant_id, name, kind, spec_text, created_at],
            )?;
            touch_tenant_tool_change(conn, tenant_id)?;
            Ok(())
        })
        .await
    }

    pub async fn list_tools(&self, tenant_id: i64) -> Result<Vec<ToolRow>, AppError> {
        self.with_conn(move |conn| {
            let sql = format!("SELECT {TOOL_COLUMNS} FROM tools WHERE tenant_id = ?1 ORDER BY name");
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map(params![tenant_id], tool_from_row)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    // ---- sharing (PRD-mcphost-sharing) ------------------------------------

    /// How many of `tenant_id`'s tools currently have a non-private
    /// visibility -- the count `host.tool_share` checks against
    /// `Plan::shared_tools_max` before allowing one more (AC6).
    pub async fn count_shared_tools(&self, tenant_id: i64) -> Result<i64, AppError> {
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM tools WHERE tenant_id = ?1 AND visibility != 'private'",
                params![tenant_id],
                |r| r.get(0),
            )
            .map_err(AppError::from)
        })
        .await
    }

    /// `host.tool_share`: set `name`'s visibility to `'public'` or
    /// `'group'` (with `group` naming the group for the latter),
    /// `share_description`, and `shared_unix = now`; clears any earlier
    /// `unshared_by` marker (a re-share is not an unshare). Returns `false`
    /// if `name` isn't one of `tenant_id`'s tools.
    // Six parameters: one atomic `UPDATE` with a single caller
    // (`sharing::tool_share`) -- same shape as `try_admit_signup` above.
    #[allow(clippy::too_many_arguments)]
    pub async fn set_tool_share(
        &self,
        tenant_id: i64,
        name: String,
        visibility: String,
        group: Option<String>,
        description: Option<String>,
    ) -> Result<bool, AppError> {
        let shared_unix = now_unix();
        self.with_conn(move |conn| {
            let n = conn.execute(
                "UPDATE tools SET visibility = ?1, shared_group = ?2, share_description = ?3, \
                 shared_unix = ?4, unshared_by = NULL \
                 WHERE tenant_id = ?5 AND name = ?6",
                params![visibility, group, description, shared_unix, tenant_id, name],
            )?;
            Ok(n > 0)
        })
        .await
    }

    /// `host.tool_unshare`: back to `'private'`, clearing the group/
    /// description/shared_unix. `unshared_by` is left untouched here (the
    /// owner unsharing its own tool is not the AC9 admin-override case);
    /// [`Self::admin_unshare_tool`] is the one that stamps it. Returns
    /// `false` if `name` isn't one of `tenant_id`'s tools.
    pub async fn unshare_tool(&self, tenant_id: i64, name: String) -> Result<bool, AppError> {
        self.with_conn(move |conn| {
            let n = conn.execute(
                "UPDATE tools SET visibility = 'private', shared_group = NULL, \
                 share_description = NULL, shared_unix = NULL \
                 WHERE tenant_id = ?1 AND name = ?2",
                params![tenant_id, name],
            )?;
            Ok(n > 0)
        })
        .await
    }

    /// `admin.tool_unshare` (AC9): same effect as [`Self::unshare_tool`],
    /// keyed by the owner's namespace (an operator names a tenant, not an
    /// id) and stamping `unshared_by = 'admin'` so the owner's own
    /// `host.tool_list` shows why its tool went private. Returns `false`
    /// if the namespace or tool name doesn't resolve to a shared tool.
    pub async fn admin_unshare_tool(
        &self,
        owner_namespace: String,
        name: String,
    ) -> Result<bool, AppError> {
        self.with_conn(move |conn| {
            let n = conn.execute(
                "UPDATE tools SET visibility = 'private', shared_group = NULL, \
                 share_description = NULL, shared_unix = NULL, unshared_by = 'admin' \
                 WHERE name = ?2 AND tenant_id = (SELECT id FROM tenants WHERE namespace = ?1)",
                params![owner_namespace, name],
            )?;
            Ok(n > 0)
        })
        .await
    }

    /// The cross-tenant resolution primitive (AC1-3): `owner_namespace`'s
    /// tool `local_name`, if it exists -- visibility is decided by the
    /// caller (`handler.rs`'s `call_shared_tool`), not here, so this never
    /// leaks "the tool exists but is private" through its `Result` shape.
    pub async fn get_tool_by_owner_namespace(
        &self,
        owner_namespace: String,
        local_name: String,
    ) -> Result<Option<(Tenant, ToolRow)>, AppError> {
        let Some(owner) = self.find_tenant_by_namespace(owner_namespace).await? else {
            return Ok(None);
        };
        let tool = self.get_tool(owner.id, local_name).await?;
        Ok(tool.map(|t| (owner, t)))
    }

    /// `host.catalog.search(q, limit)` (AC7): a `LIKE` over name and
    /// `share_description` among `visibility = 'public'` tools, joined to
    /// the owner's namespace. `q` of `None` (or empty) returns every public
    /// tool, newest-shared first.
    pub async fn search_catalog(
        &self,
        q: Option<String>,
        limit: i64,
    ) -> Result<Vec<(String, ToolRow)>, AppError> {
        self.with_conn(move |conn| {
            let pattern = q
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(|s| format!("%{s}%"));
            let cols: Vec<String> = TOOL_COLUMNS
                .split(", ")
                .map(|c| format!("tools.{c}"))
                .collect();
            let sql = format!(
                "SELECT tenants.namespace, {} FROM tools JOIN tenants ON tenants.id = tools.tenant_id \
                 WHERE tools.visibility = 'public' \
                 AND (?1 IS NULL OR tools.name LIKE ?1 OR tools.share_description LIKE ?1) \
                 ORDER BY tools.shared_unix DESC LIMIT ?2",
                cols.join(", ")
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map(params![pattern, limit], |r| {
                    let namespace: String = r.get(0)?;
                    // tool_from_row expects the TOOL_COLUMNS to start at
                    // index 0; here they start at index 1 because of the
                    // leading `tenants.namespace` column, so build a
                    // one-off Row-shifted reader instead of reusing it.
                    let spec_text: String = r.get(5)?;
                    Ok((
                        namespace,
                        ToolRow {
                            id: r.get(1)?,
                            tenant_id: r.get(2)?,
                            name: r.get(3)?,
                            kind: r.get(4)?,
                            spec: serde_json::from_str(&spec_text).unwrap_or(Value::Null),
                            created_at: r.get(6)?,
                            visibility: r.get(7)?,
                            share_description: r.get(8)?,
                            shared_unix: r.get(9)?,
                            shared_group: r.get(10)?,
                            unshared_by: r.get(11)?,
                        },
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// `host.catalog.get(full_name)` / the well-known catalog document: one
    /// public tool by its `<namespace>.<name>` full name, or `None` if it
    /// doesn't exist or isn't public.
    pub async fn get_public_tool(
        &self,
        owner_namespace: String,
        local_name: String,
    ) -> Result<Option<ToolRow>, AppError> {
        let Some((_, tool)) = self
            .get_tool_by_owner_namespace(owner_namespace, local_name)
            .await?
        else {
            return Ok(None);
        };
        Ok((tool.visibility == "public").then_some(tool))
    }

    // ---- groups (PRD-mcphost-sharing) -------------------------------------

    /// `host.group.create`: idempotent -- creating an already-existing
    /// group name for this owner is a no-op, not an error (mirrors
    /// `upsert_tool`'s republish-is-fine convention).
    pub async fn create_group(&self, owner_tenant_id: i64, name: String) -> Result<(), AppError> {
        let created_at = now_rfc3339();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO groups (owner_tenant_id, name, created_at) VALUES (?1, ?2, ?3) \
                 ON CONFLICT(owner_tenant_id, name) DO NOTHING",
                params![owner_tenant_id, name, created_at],
            )?;
            Ok(())
        })
        .await
    }

    fn find_group_id_sync(
        conn: &Connection,
        owner_tenant_id: i64,
        name: &str,
    ) -> Result<Option<i64>, AppError> {
        conn.query_row(
            "SELECT id FROM groups WHERE owner_tenant_id = ?1 AND name = ?2",
            params![owner_tenant_id, name],
            |r| r.get(0),
        )
        .optional()
        .map_err(AppError::from)
    }

    /// `host.group.add`: add `member_tenant_id` to `owner_tenant_id`'s
    /// group `name`. Returns `false` if the group doesn't exist yet (the
    /// caller must `host.group.create` first).
    pub async fn group_add_member(
        &self,
        owner_tenant_id: i64,
        name: String,
        member_tenant_id: i64,
    ) -> Result<bool, AppError> {
        let created_at = now_rfc3339();
        self.with_conn(move |conn| {
            let Some(group_id) = Self::find_group_id_sync(conn, owner_tenant_id, &name)? else {
                return Ok(false);
            };
            conn.execute(
                "INSERT INTO group_members (group_id, member_tenant_id, created_at) \
                 VALUES (?1, ?2, ?3) ON CONFLICT(group_id, member_tenant_id) DO NOTHING",
                params![group_id, member_tenant_id, created_at],
            )?;
            Ok(true)
        })
        .await
    }

    /// `host.group.remove`: drop `member_tenant_id` from the group.
    /// Returns `false` if the group doesn't exist; removing a member who
    /// was never in it is a no-op success (same idempotent convention as
    /// `create_group`).
    pub async fn group_remove_member(
        &self,
        owner_tenant_id: i64,
        name: String,
        member_tenant_id: i64,
    ) -> Result<bool, AppError> {
        self.with_conn(move |conn| {
            let Some(group_id) = Self::find_group_id_sync(conn, owner_tenant_id, &name)? else {
                return Ok(false);
            };
            conn.execute(
                "DELETE FROM group_members WHERE group_id = ?1 AND member_tenant_id = ?2",
                params![group_id, member_tenant_id],
            )?;
            Ok(true)
        })
        .await
    }

    /// `host.group.list`: every group this tenant owns, each with its
    /// member namespaces.
    pub async fn list_groups(&self, owner_tenant_id: i64) -> Result<Vec<(String, Vec<String>)>, AppError> {
        self.with_conn(move |conn| {
            let mut group_stmt = conn.prepare(
                "SELECT id, name FROM groups WHERE owner_tenant_id = ?1 ORDER BY name",
            )?;
            let groups = group_stmt
                .query_map(params![owner_tenant_id], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            let mut out = Vec::with_capacity(groups.len());
            for (group_id, name) in groups {
                let mut member_stmt = conn.prepare(
                    "SELECT tenants.namespace FROM group_members \
                     JOIN tenants ON tenants.id = group_members.member_tenant_id \
                     WHERE group_members.group_id = ?1 ORDER BY tenants.namespace",
                )?;
                let members = member_stmt
                    .query_map(params![group_id], |r| r.get(0))?
                    .collect::<Result<Vec<_>, _>>()?;
                out.push((name, members));
            }
            Ok(out)
        })
        .await
    }

    /// The resolution-time membership check (AC3): is `member_tenant_id` a
    /// member of `owner_tenant_id`'s group `name`?
    pub async fn is_group_member(
        &self,
        owner_tenant_id: i64,
        name: String,
        member_tenant_id: i64,
    ) -> Result<bool, AppError> {
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT 1 FROM groups JOIN group_members ON group_members.group_id = groups.id \
                 WHERE groups.owner_tenant_id = ?1 AND groups.name = ?2 \
                 AND group_members.member_tenant_id = ?3",
                params![owner_tenant_id, name, member_tenant_id],
                |_| Ok(()),
            )
            .optional()
            .map(|r| r.is_some())
            .map_err(AppError::from)
        })
        .await
    }

    /// `admin.shared_tools`: every currently-shared (non-private) tool
    /// across every tenant, with its owner namespace and per-day caller
    /// counts -- the per-day breakdown mirrors [`Self::calls_by_others`]
    /// but across all owners rather than one, and grouped by day rather
    /// than lifetime.
    pub async fn admin_shared_tools(&self) -> Result<Vec<Value>, AppError> {
        self.with_conn(move |conn| {
            let sql = format!(
                "SELECT tenants.namespace, {} FROM tools JOIN tenants ON tenants.id = tools.tenant_id \
                 WHERE tools.visibility != 'private' ORDER BY tools.shared_unix DESC",
                TOOL_COLUMNS
                    .split(", ")
                    .map(|c| format!("tools.{c}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map([], |r| {
                    let namespace: String = r.get(0)?;
                    let spec_text: String = r.get(5)?;
                    let tool = ToolRow {
                        id: r.get(1)?,
                        tenant_id: r.get(2)?,
                        name: r.get(3)?,
                        kind: r.get(4)?,
                        spec: serde_json::from_str(&spec_text).unwrap_or(Value::Null),
                        created_at: r.get(6)?,
                        visibility: r.get(7)?,
                        share_description: r.get(8)?,
                        shared_unix: r.get(9)?,
                        shared_group: r.get(10)?,
                        unshared_by: r.get(11)?,
                    };
                    Ok((namespace, tool))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            let mut out = Vec::with_capacity(rows.len());
            for (namespace, tool) in rows {
                let mut caller_stmt = conn.prepare(
                    "SELECT COALESCE(t2.namespace, '?'), DATE(calls.started_at) AS day, COUNT(*) \
                     FROM calls JOIN tenants t2 ON t2.id = calls.caller_tenant_id \
                     WHERE calls.tenant_id = ?1 AND calls.tool_name = ?2 AND calls.caller_tenant_id IS NOT NULL \
                     GROUP BY t2.namespace, day ORDER BY day DESC",
                )?;
                let callers = caller_stmt
                    .query_map(params![tool.tenant_id, tool.name], |r| {
                        Ok(json!({
                            "caller": r.get::<_, String>(0)?,
                            "day": r.get::<_, String>(1)?,
                            "calls": r.get::<_, i64>(2)?,
                        }))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                out.push(json!({
                    "name": format!("{namespace}.{}", tool.name),
                    "owner": namespace,
                    "visibility": tool.visibility,
                    "group": tool.shared_group,
                    "description": tool.share_description,
                    "callers": callers,
                }));
            }
            Ok(out)
        })
        .await
    }

    /// `host.usage`'s `calls_to_shared` (AC5): how many calls this tenant
    /// made, as caller, to another tenant's shared tool, in `window_secs`.
    pub async fn calls_to_shared(&self, tenant_id: i64, window_secs: i64) -> Result<i64, AppError> {
        let since = now_unix() - window_secs;
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM calls WHERE caller_tenant_id = ?1 AND started_unix >= ?2",
                params![tenant_id, since],
                |r| r.get(0),
            )
            .map_err(AppError::from)
        })
        .await
    }

    /// `host.usage`'s `calls_by_others` (AC5): calls this tenant's tools
    /// received from OTHER tenants in `window_secs`, grouped by caller
    /// namespace.
    pub async fn calls_by_others(
        &self,
        tenant_id: i64,
        window_secs: i64,
    ) -> Result<Vec<(String, i64)>, AppError> {
        let since = now_unix() - window_secs;
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT tenants.namespace, COUNT(*) FROM calls \
                 JOIN tenants ON tenants.id = calls.caller_tenant_id \
                 WHERE calls.tenant_id = ?1 AND calls.caller_tenant_id IS NOT NULL \
                 AND calls.started_unix >= ?2 GROUP BY tenants.namespace",
            )?;
            let rows = stmt
                .query_map(params![tenant_id, since], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    pub async fn remove_tool(&self, tenant_id: i64, name: String) -> Result<bool, AppError> {
        self.with_conn(move |conn| {
            let n = conn.execute(
                "DELETE FROM tools WHERE tenant_id = ?1 AND name = ?2",
                params![tenant_id, name],
            )?;
            if n > 0 {
                // AC18: a remove is a tool-set change exactly like a publish
                // (PRD requirement 14 says "publish or remove"), but it
                // deletes the row a naive max(created_at)-over-surviving-rows
                // read would need -- so the tenant carries its own stamp.
                touch_tenant_tool_change(conn, tenant_id)?;
            }
            Ok(n > 0)
        })
        .await
    }

    // ---- secrets ---------------------------------------------------------

    pub async fn upsert_secret(
        &self,
        tenant_id: i64,
        name: String,
        value_enc: Vec<u8>,
        nonce: Vec<u8>,
    ) -> Result<(), AppError> {
        let created_at = now_rfc3339();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO secrets (tenant_id, name, value_enc, nonce, created_at) VALUES (?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT(tenant_id, name) DO UPDATE SET value_enc = excluded.value_enc, nonce = excluded.nonce",
                params![tenant_id, name, value_enc, nonce, created_at],
            )?;
            Ok(())
        })
        .await
    }

    /// PRD-grand-loop-billing: `secrets_max` enforcement in
    /// `control::secret_set`.
    pub async fn count_secrets(&self, tenant_id: i64) -> Result<i64, AppError> {
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM secrets WHERE tenant_id = ?1",
                params![tenant_id],
                |r| r.get(0),
            )
            .map_err(AppError::from)
        })
        .await
    }

    pub async fn list_secret_names(&self, tenant_id: i64) -> Result<Vec<String>, AppError> {
        self.with_conn(move |conn| {
            let mut stmt =
                conn.prepare("SELECT name FROM secrets WHERE tenant_id = ?1 ORDER BY name")?;
            let rows = stmt
                .query_map(params![tenant_id], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    pub async fn get_secret(
        &self,
        tenant_id: i64,
        name: String,
    ) -> Result<Option<(Vec<u8>, Vec<u8>)>, AppError> {
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT value_enc, nonce FROM secrets WHERE tenant_id = ?1 AND name = ?2",
                params![tenant_id, name],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(AppError::from)
        })
        .await
    }

    // ---- tenant state (PRD-mcphost-tenant-state P0 requirement 1) ------
    //
    // `tenant_state::business` (this crate's new `tenant_state.rs`) owns
    // schema validation, the filter grammar and quota arithmetic; these
    // methods are the same thin "one prepared statement, one shape" layer
    // every other section of this file already is.

    pub async fn state_kv_set(
        &self,
        tenant_id: i64,
        key: String,
        value_json: String,
    ) -> Result<(), AppError> {
        let updated_unix = now_unix();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO tenant_state_kv (tenant_id, key, value_json, updated_unix) \
                 VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT(tenant_id, key) DO UPDATE SET \
                     value_json = excluded.value_json, updated_unix = excluded.updated_unix",
                params![tenant_id, key, value_json, updated_unix],
            )?;
            Ok(())
        })
        .await
    }

    /// `(value_json, updated_unix)`, `None` if the key has never been set
    /// (or was deleted) for this tenant.
    pub async fn state_kv_get(
        &self,
        tenant_id: i64,
        key: String,
    ) -> Result<Option<(String, i64)>, AppError> {
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT value_json, updated_unix FROM tenant_state_kv \
                 WHERE tenant_id = ?1 AND key = ?2",
                params![tenant_id, key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(AppError::from)
        })
        .await
    }

    pub async fn state_kv_delete(&self, tenant_id: i64, key: String) -> Result<bool, AppError> {
        self.with_conn(move |conn| {
            let n = conn.execute(
                "DELETE FROM tenant_state_kv WHERE tenant_id = ?1 AND key = ?2",
                params![tenant_id, key],
            )?;
            Ok(n > 0)
        })
        .await
    }

    /// `AC1`/goal 1's `host.state.list`: every `(key, value_json,
    /// updated_unix)` for this tenant whose key starts with `prefix` (all
    /// keys when `prefix` is `None`), ordered by key, capped at `limit`.
    pub async fn state_kv_list(
        &self,
        tenant_id: i64,
        prefix: Option<String>,
        limit: i64,
    ) -> Result<Vec<(String, String, i64)>, AppError> {
        self.with_conn(move |conn| {
            let pattern = prefix
                .as_deref()
                .map(|p| format!("{}%", p.replace('%', "\\%").replace('_', "\\_")));
            let mut stmt = conn.prepare(
                "SELECT key, value_json, updated_unix FROM tenant_state_kv \
                 WHERE tenant_id = ?1 AND (?2 IS NULL OR key LIKE ?2 ESCAPE '\\') \
                 ORDER BY key LIMIT ?3",
            )?;
            let rows = stmt
                .query_map(params![tenant_id, pattern, limit], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// Sum of stored byte lengths across a tenant's whole state (both the
    /// KV namespace and every declared table's rows) -- the quota-check
    /// input requirement 4 / the technical considerations pin to "stored
    /// byte lengths, not `PRAGMA page_count`".
    pub async fn state_bytes_used(&self, tenant_id: i64) -> Result<i64, AppError> {
        self.with_conn(move |conn| {
            let kv_bytes: i64 = conn.query_row(
                "SELECT COALESCE(SUM(LENGTH(value_json)), 0) FROM tenant_state_kv \
                 WHERE tenant_id = ?1",
                params![tenant_id],
                |r| r.get(0),
            )?;
            let row_bytes: i64 = conn.query_row(
                "SELECT COALESCE(SUM(LENGTH(row_json)), 0) FROM tenant_state_rows \
                 WHERE tenant_id = ?1",
                params![tenant_id],
                |r| r.get(0),
            )?;
            Ok(kv_bytes + row_bytes)
        })
        .await
    }

    /// `(schema_json, primary_key)` for a declared table, `None` if this
    /// tenant has no table by that name.
    pub async fn state_table_get(
        &self,
        tenant_id: i64,
        table_name: String,
    ) -> Result<Option<(String, Option<String>)>, AppError> {
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT schema_json, primary_key FROM tenant_state_tables \
                 WHERE tenant_id = ?1 AND table_name = ?2",
                params![tenant_id, table_name],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(AppError::from)
        })
        .await
    }

    /// `host.state.table_create`: upsert semantics, same rationale as
    /// `upsert_secret` -- a republish of an already-declared table (e.g.
    /// widening a schema) replaces rather than errors.
    pub async fn state_table_create(
        &self,
        tenant_id: i64,
        table_name: String,
        schema_json: String,
        primary_key: Option<String>,
    ) -> Result<(), AppError> {
        let created_unix = now_unix();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO tenant_state_tables (tenant_id, table_name, schema_json, \
                     primary_key, created_unix) VALUES (?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT(tenant_id, table_name) DO UPDATE SET \
                     schema_json = excluded.schema_json, primary_key = excluded.primary_key",
                params![tenant_id, table_name, schema_json, primary_key, created_unix],
            )?;
            Ok(())
        })
        .await
    }

    /// Drops the table registration and every row it owns, in one
    /// transaction. `true` if a table by that name existed.
    pub async fn state_table_drop(
        &self,
        tenant_id: i64,
        table_name: String,
    ) -> Result<bool, AppError> {
        self.with_conn(move |conn| {
            conn.execute("BEGIN IMMEDIATE", []).map_err(AppError::from)?;
            let outcome: Result<bool, AppError> = (|| {
                let n = conn.execute(
                    "DELETE FROM tenant_state_tables WHERE tenant_id = ?1 AND table_name = ?2",
                    params![tenant_id, table_name],
                )?;
                conn.execute(
                    "DELETE FROM tenant_state_rows WHERE tenant_id = ?1 AND table_name = ?2",
                    params![tenant_id, table_name],
                )?;
                Ok(n > 0)
            })();
            match outcome {
                Ok(existed) => {
                    conn.execute("COMMIT", []).map_err(AppError::from)?;
                    Ok(existed)
                }
                Err(e) => {
                    let _ = conn.execute("ROLLBACK", []);
                    Err(e)
                }
            }
        })
        .await
    }

    /// Every `(id, row_json)` currently stored for `table_name` -- the
    /// input `tenant_state::query_table`/`delete_rows_where` filter over in
    /// Rust (see migration 0011's doc comment for why this isn't
    /// pushed-down SQL).
    pub async fn state_rows_all(
        &self,
        tenant_id: i64,
        table_name: String,
    ) -> Result<Vec<(i64, String)>, AppError> {
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, row_json FROM tenant_state_rows \
                 WHERE tenant_id = ?1 AND table_name = ?2 ORDER BY id",
            )?;
            let rows = stmt
                .query_map(params![tenant_id, table_name], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    pub async fn state_row_count(&self, tenant_id: i64, table_name: String) -> Result<i64, AppError> {
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM tenant_state_rows WHERE tenant_id = ?1 AND table_name = ?2",
                params![tenant_id, table_name],
                |r| r.get(0),
            )
            .map_err(AppError::from)
        })
        .await
    }

    pub async fn state_row_insert(
        &self,
        tenant_id: i64,
        table_name: String,
        row_json: String,
    ) -> Result<i64, AppError> {
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO tenant_state_rows (tenant_id, table_name, row_json) \
                 VALUES (?1, ?2, ?3)",
                params![tenant_id, table_name, row_json],
            )?;
            Ok(conn.last_insert_rowid())
        })
        .await
    }

    pub async fn state_rows_delete_by_ids(
        &self,
        tenant_id: i64,
        table_name: String,
        ids: Vec<i64>,
    ) -> Result<i64, AppError> {
        if ids.is_empty() {
            return Ok(0);
        }
        self.with_conn(move |conn| {
            let mut deleted = 0i64;
            for id in ids {
                deleted += conn.execute(
                    "DELETE FROM tenant_state_rows \
                     WHERE tenant_id = ?1 AND table_name = ?2 AND id = ?3",
                    params![tenant_id, table_name, id],
                )? as i64;
            }
            Ok(deleted)
        })
        .await
    }

    // ---- registry publish (AC19) ---------------------------------------

    /// Store (or replace) a tenant's most recently published `server.json`
    /// document, keyed by tenant so a re-publish overwrites rather than
    /// accumulates. Called only after the registry API itself has accepted
    /// the document (`control::registry_publish`), so what's stored here is
    /// always what's actually live at the registry.
    pub async fn upsert_registry_document(
        &self,
        tenant_id: i64,
        namespace: String,
        document: Value,
    ) -> Result<(), AppError> {
        let published_at = now_rfc3339();
        let doc_text = serde_json::to_string(&document)
            .map_err(|e| AppError::Internal(format!("registry document serialize: {e}")))?;
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO registry_documents (tenant_id, namespace, document, published_at) \
                 VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT(tenant_id) DO UPDATE SET namespace = excluded.namespace, \
                     document = excluded.document, published_at = excluded.published_at",
                params![tenant_id, namespace, doc_text, published_at],
            )?;
            Ok(())
        })
        .await
    }

    /// Read back a tenant's published `server.json` by their (mcphost, not
    /// domain) namespace -- what `GET /.well-known/mcp/<namespace>/server.json`
    /// serves. `None` if that tenant has never successfully published.
    pub async fn get_registry_document(
        &self,
        namespace: String,
    ) -> Result<Option<Value>, AppError> {
        self.with_conn(move |conn| {
            let doc_text: Option<String> = conn
                .query_row(
                    "SELECT document FROM registry_documents WHERE namespace = ?1",
                    params![namespace],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(doc_text.and_then(|text| serde_json::from_str(&text).ok()))
        })
        .await
    }

    // ---- calls / metering ------------------------------------------------

    // Iteration 1 (rustbuild Stage 3, 2026-09-02): clippy.toml's scaffolded
    // too-many-arguments-threshold (5) is tighter than the default (7) this
    // function was written against; #[allow] here is a targeted, minimal
    // fix rather than reshaping a working, already-tested call signature.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    pub async fn record_call(
        &self,
        tenant_id: i64,
        tool_name: String,
        duration_ms: i64,
        ok: bool,
        error_class: Option<String>,
        cpu_ms: Option<i64>,
        peak_rss_kb: Option<i64>,
        outcome: &str,
        origin: String,
        origin_detail: Option<String>,
    ) -> Result<(), AppError> {
        self.record_call_attributed(
            tenant_id,
            tool_name,
            duration_ms,
            ok,
            error_class,
            cpu_ms,
            peak_rss_kb,
            outcome,
            origin,
            origin_detail,
            None,
        )
        .await
    }

    /// PRD-mcphost-sharing requirement 3 (AC1): same as [`Self::record_call`],
    /// plus `caller_tenant_id` -- set only for a cross-tenant call
    /// (`handler.rs`'s `call_shared_tool`), `None` for every same-tenant
    /// call, which [`Self::record_call`] above still covers with no call
    /// site changes. Kept as a second function (rather than adding a
    /// required 11th parameter to `record_call`, whose 10 are already
    /// `#[allow(clippy::too_many_arguments)]`) so the many existing
    /// same-tenant call sites (tests included) don't all need a trailing
    /// `None`.
    #[allow(clippy::too_many_arguments)]
    pub async fn record_call_attributed(
        &self,
        tenant_id: i64,
        tool_name: String,
        duration_ms: i64,
        ok: bool,
        error_class: Option<String>,
        cpu_ms: Option<i64>,
        peak_rss_kb: Option<i64>,
        outcome: &str,
        origin: String,
        origin_detail: Option<String>,
        caller_tenant_id: Option<i64>,
    ) -> Result<(), AppError> {
        let started_at = now_rfc3339();
        let started_unix = now_unix();
        let outcome = outcome.to_string();
        // PRD-mcphost-runs-and-jobs P0 requirement 2: every synchronous call
        // also writes a `runs` row with `trigger='call'`, in the SAME
        // transaction as its `calls` row -- so the ledger is complete from
        // day one and a crash between the two inserts is impossible (both
        // land, or neither does). `calls` itself is untouched (unmodified
        // columns, unmodified insert above this comment).
        let run_id = crate::state::new_ulid();
        let run_status = match (ok, outcome.as_str()) {
            (true, _) => "done",
            (false, "call_timeout") => "timeout",
            (false, _) => "error",
        };
        let run_error_class = error_class.clone();
        let run_tool_name = tool_name.clone();
        self.with_conn(move |conn| {
            let tx = conn.unchecked_transaction()?;
            tx.execute(
                "INSERT INTO calls (tenant_id, tool_name, started_at, started_unix, duration_ms, ok, error_class, cpu_ms, peak_rss_kb, outcome, origin, origin_detail, caller_tenant_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![tenant_id, tool_name, started_at, started_unix, duration_ms, ok as i64, error_class, cpu_ms, peak_rss_kb, outcome, origin, origin_detail, caller_tenant_id],
            )?;
            tx.execute(
                "INSERT INTO runs (id, tenant_id, tool_name, trigger, trigger_ref, caller_tenant_id, \
                 status, progress_json, result_ref, error_class, started_unix, finished_unix, \
                 duration_ms, deadline_s, attempt) \
                 VALUES (?1, ?2, ?3, 'call', NULL, ?4, ?5, NULL, NULL, ?6, ?7, ?8, ?9, NULL, 1)",
                params![
                    run_id,
                    tenant_id,
                    run_tool_name,
                    caller_tenant_id,
                    run_status,
                    run_error_class,
                    started_unix,
                    started_unix + (duration_ms / 1000).max(0),
                    duration_ms,
                ],
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    // ---- runs ledger (PRD-mcphost-runs-and-jobs) --------------------------

    /// P0 requirement 3: inserts a `queued` run and returns its new id, for
    /// `host.tool_call(..., async=true)` -- must be fast (AC1: "under 50
    /// ms"), so this is a single-row insert with no read-back.
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_queued_run(
        &self,
        run_id: String,
        tenant_id: i64,
        tool_name: String,
        trigger: String,
        trigger_ref: Option<String>,
        caller_tenant_id: Option<i64>,
        deadline_s: i64,
        args_json: String,
        // PRD-mcphost-schedules P1 requirement 7 / migration 0016: `true`
        // only for `host.trigger.fire`'s own enqueue (AC10); every other
        // caller (an ordinary async call, the scheduler tick) passes `false`.
        manual: bool,
    ) -> Result<(), AppError> {
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO runs (id, tenant_id, tool_name, trigger, trigger_ref, \
                 caller_tenant_id, status, deadline_s, attempt, args_json, manual) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'queued', ?7, 1, ?8, ?9)",
                params![run_id, tenant_id, tool_name, trigger, trigger_ref, caller_tenant_id, deadline_s, args_json, manual],
            )?;
            Ok(())
        })
        .await
    }

    /// PRD-mcphost-schedules P0 requirement 3: records a firing the
    /// scheduler tick skipped because the trigger's previous run was still
    /// `queued`/`running` (AC5) -- inserted already-terminal (`skipped`),
    /// never leased by the executor.
    pub async fn insert_skipped_run(
        &self,
        run_id: String,
        tenant_id: i64,
        tool_name: String,
        trigger_ref: String,
    ) -> Result<(), AppError> {
        let now = now_unix();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO runs (id, tenant_id, tool_name, trigger, trigger_ref, status, \
                 error_class, started_unix, finished_unix, duration_ms, attempt) \
                 VALUES (?1, ?2, ?3, 'schedule', ?4, 'skipped', 'overlap', ?5, ?5, 0, 1)",
                params![run_id, tenant_id, tool_name, trigger_ref, now],
            )?;
            Ok(())
        })
        .await
    }

    /// Technical considerations' leasing SQL, scoped to one tenant so the
    /// executor's per-tenant `jobs_concurrent` admission check (done by the
    /// caller, before this is called) and this lease can never race each
    /// other into over-admitting that tenant. `RETURNING id` under the
    /// write lock is enough at this scale (single-writer SQLite via
    /// `with_conn`'s own mutex); `None` when this tenant has no `queued`
    /// row right now.
    pub async fn lease_next_queued_run(&self, tenant_id: i64) -> Result<Option<RunRow>, AppError> {
        let now = now_unix();
        self.with_conn(move |conn| {
            // `ORDER BY rowid`, not `ORDER BY id`: `id` is a ulid (see
            // `state::new_ulid`) whose own doc comment admits two ids
            // minted in the same millisecond can tie on their time prefix
            // and sort by random suffix instead -- AC6's "second job stays
            // queued" test caught exactly that (two `async=true` enqueues
            // milliseconds apart occasionally leased out of submission
            // order). `rowid` is SQLite's own monotonically-increasing
            // insertion counter for this table (no `WITHOUT ROWID`), so it
            // gives true FIFO regardless of ulid collisions.
            let leased_id: Option<String> = conn
                .query_row(
                    "UPDATE runs SET status='running', started_unix=?1 \
                     WHERE id = (SELECT id FROM runs WHERE status='queued' AND tenant_id=?2 ORDER BY rowid LIMIT 1) \
                     RETURNING id",
                    params![now, tenant_id],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(id) = leased_id else {
                return Ok(None);
            };
            let row = conn.query_row(
                &format!("SELECT {RUN_COLUMNS} FROM runs WHERE id = ?1"),
                params![id],
                run_row_from_row,
            )?;
            Ok(Some(row))
        })
        .await
    }

    /// The executor's per-tenant admission check (P0 requirement 4:
    /// `jobs_concurrent`) -- how many of this tenant's runs are `running`
    /// right now, any trigger (a job sharing the tenant's slot budget with
    /// nothing else today, but the column isn't trigger-filtered so a
    /// future trigger kind doesn't silently bypass the cap).
    pub async fn count_running_runs_for_tenant(&self, tenant_id: i64) -> Result<i64, AppError> {
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM runs WHERE tenant_id = ?1 AND status = 'running'",
                params![tenant_id],
                |r| r.get(0),
            )
            .map_err(AppError::from)
        })
        .await
    }

    /// The executor's host-wide ceiling check, independent of any one
    /// tenant's own cap.
    pub async fn count_running_runs_total(&self) -> Result<i64, AppError> {
        self.with_conn(|conn| {
            conn.query_row("SELECT COUNT(*) FROM runs WHERE status = 'running'", [], |r| {
                r.get(0)
            })
            .map_err(AppError::from)
        })
        .await
    }

    /// Every tenant with at least one `queued` run right now -- the
    /// executor's outer scan loop iterates this list each tick rather than
    /// scanning every tenant that has ever existed.
    pub async fn distinct_tenants_with_queued_runs(&self) -> Result<Vec<i64>, AppError> {
        self.with_conn(|conn| {
            let mut stmt =
                conn.prepare("SELECT DISTINCT tenant_id FROM runs WHERE status = 'queued'")?;
            let rows = stmt
                .query_map([], |r| r.get::<_, i64>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// P0 requirement 5: the executor's throttled (at most once/second,
    /// enforced by the caller) progress write. A no-op (0 rows affected,
    /// not an error) if the run has already left `running` -- a stray
    /// progress line arriving after cancellation/finalization must never
    /// resurrect a terminal row.
    pub async fn update_run_progress(
        &self,
        run_id: String,
        tenant_id: i64,
        progress_json: String,
    ) -> Result<(), AppError> {
        self.with_conn(move |conn| {
            conn.execute(
                "UPDATE runs SET progress_json = ?1 WHERE id = ?2 AND tenant_id = ?3 AND status = 'running'",
                params![progress_json, run_id, tenant_id],
            )?;
            Ok(())
        })
        .await
    }

    /// Finalizes a run to a terminal status (`done`/`error`/`timeout`) --
    /// conditional on the row still being `running` OR `queued` (a queued
    /// job whose tool vanished before ever leasing is also finalized
    /// through here), so a run `host.runs.cancel` already flipped to
    /// `cancelled` is never overwritten by the executor's own finalize
    /// racing in after the kill. Returns `true` if this call is the one
    /// that actually finalized it (the caller uses this to decide whether
    /// to also write the state-store result -- a lost race writes nothing).
    #[allow(clippy::too_many_arguments)]
    pub async fn finalize_run(
        &self,
        run_id: String,
        tenant_id: i64,
        status: String,
        result_ref: Option<String>,
        error_class: Option<String>,
        finished_unix: i64,
        duration_ms: i64,
    ) -> Result<bool, AppError> {
        self.with_conn(move |conn| {
            let affected = conn.execute(
                "UPDATE runs SET status = ?1, result_ref = ?2, error_class = ?3, \
                 finished_unix = ?4, duration_ms = ?5 \
                 WHERE id = ?6 AND tenant_id = ?7 AND status IN ('running', 'queued')",
                params![status, result_ref, error_class, finished_unix, duration_ms, run_id, tenant_id],
            )?;
            Ok(affected > 0)
        })
        .await
    }

    /// `host.runs.cancel` (AC4): conditional on the row still being
    /// `queued` or `running`; returns the row as it stood BEFORE this call
    /// (so the caller can tell whether it was actually `running`, i.e.
    /// worth reaching for a live pid to kill) when this call is the one
    /// that flipped it, `None` if it was already terminal (nothing to
    /// cancel).
    pub async fn cancel_run(&self, run_id: String, tenant_id: i64) -> Result<Option<RunRow>, AppError> {
        let now = now_unix();
        self.with_conn(move |conn| {
            let before = conn
                .query_row(
                    &format!("SELECT {RUN_COLUMNS} FROM runs WHERE id = ?1 AND tenant_id = ?2"),
                    params![run_id, tenant_id],
                    run_row_from_row,
                )
                .optional()?;
            let Some(before) = before else {
                return Ok(None);
            };
            if before.status != "queued" && before.status != "running" {
                return Ok(None);
            }
            let affected = conn.execute(
                "UPDATE runs SET status = 'cancelled', finished_unix = ?1 \
                 WHERE id = ?2 AND tenant_id = ?3 AND status IN ('queued', 'running')",
                params![now, run_id, tenant_id],
            )?;
            if affected == 0 {
                return Ok(None); // lost a race with the executor's own finalize.
            }
            Ok(Some(before))
        })
        .await
    }

    /// `host.runs.get`/`host.runs.wait`.
    pub async fn get_run(&self, run_id: String, tenant_id: i64) -> Result<Option<RunRow>, AppError> {
        self.with_conn(move |conn| {
            conn.query_row(
                &format!("SELECT {RUN_COLUMNS} FROM runs WHERE id = ?1 AND tenant_id = ?2"),
                params![run_id, tenant_id],
                run_row_from_row,
            )
            .optional()
            .map_err(AppError::from)
        })
        .await
    }

    /// `host.runs.list(tool?, status?, trigger?, limit?)` -- newest first.
    #[allow(clippy::too_many_arguments)]
    pub async fn list_runs(
        &self,
        tenant_id: i64,
        tool_name: Option<String>,
        status: Option<String>,
        trigger: Option<String>,
        limit: i64,
    ) -> Result<Vec<RunRow>, AppError> {
        self.with_conn(move |conn| {
            let mut sql = format!("SELECT {RUN_COLUMNS} FROM runs WHERE tenant_id = ?1");
            let mut idx = 2;
            let mut binds: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(tenant_id)];
            if let Some(t) = tool_name {
                sql.push_str(&format!(" AND tool_name = ?{idx}"));
                binds.push(Box::new(t));
                idx += 1;
            }
            if let Some(s) = status {
                sql.push_str(&format!(" AND status = ?{idx}"));
                binds.push(Box::new(s));
                idx += 1;
            }
            if let Some(tr) = trigger {
                sql.push_str(&format!(" AND trigger = ?{idx}"));
                binds.push(Box::new(tr));
                idx += 1;
            }
            // `rowid`, not `id` (a ulid): see `lease_next_queued_run`'s
            // comment -- same-millisecond ties on `id` would otherwise
            // put "newest first" out of true insertion order.
            sql.push_str(&format!(" ORDER BY rowid DESC LIMIT ?{idx}"));
            binds.push(Box::new(limit));
            let refs: Vec<&dyn rusqlite::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map(refs.as_slice(), run_row_from_row)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// `admin.runs(tenant?, status?, limit?)` -- cross-tenant; `tenant_id`
    /// filters to one tenant when given, otherwise every tenant.
    pub async fn admin_list_runs(
        &self,
        tenant_id: Option<i64>,
        status: Option<String>,
        limit: i64,
    ) -> Result<Vec<RunRow>, AppError> {
        self.with_conn(move |conn| {
            let mut sql = format!("SELECT {RUN_COLUMNS} FROM runs WHERE 1=1");
            let mut idx = 1;
            let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
            if let Some(t) = tenant_id {
                sql.push_str(&format!(" AND tenant_id = ?{idx}"));
                binds.push(Box::new(t));
                idx += 1;
            }
            if let Some(s) = status {
                sql.push_str(&format!(" AND status = ?{idx}"));
                binds.push(Box::new(s));
                idx += 1;
            }
            sql.push_str(&format!(" ORDER BY rowid DESC LIMIT ?{idx}"));
            binds.push(Box::new(limit));
            let refs: Vec<&dyn rusqlite::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map(refs.as_slice(), run_row_from_row)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// `host.runs.purge(before_unix)` (AC7): every `done` run for this
    /// tenant finished at or before `before_unix` with a still-live
    /// `result_ref` has that ref cleared and `purged_unix` stamped;
    /// returns the list of `result_ref` keys the caller (`runs.rs`) must
    /// also delete from `tenant_state_kv` -- this function only owns the
    /// `runs` row itself, not the state store (`tenant_state.rs`'s
    /// territory, same separation `state_table_drop`'s caller already
    /// keeps in `tenant_state.rs`).
    pub async fn purge_runs(&self, tenant_id: i64, before_unix: i64) -> Result<Vec<String>, AppError> {
        let now = now_unix();
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT result_ref FROM runs WHERE tenant_id = ?1 AND status = 'done' \
                 AND finished_unix <= ?2 AND result_ref IS NOT NULL",
            )?;
            let refs: Vec<String> = stmt
                .query_map(params![tenant_id, before_unix], |r| r.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            conn.execute(
                "UPDATE runs SET result_ref = NULL, purged_unix = ?1 \
                 WHERE tenant_id = ?2 AND status = 'done' AND finished_unix <= ?3 AND result_ref IS NOT NULL",
                params![now, tenant_id, before_unix],
            )?;
            Ok(refs)
        })
        .await
    }

    /// `admin.runs_reap` (P1 requirement 10, AC11): every `running` run
    /// whose `started_unix + deadline_s` has passed with no finalization
    /// (an executor crash) reads `error` / `error_class: interrupted`.
    /// Returns the count reaped. Rows with no `deadline_s` (shouldn't
    /// happen -- every leased run's deadline is set at insert time) are
    /// left alone rather than reaped on an assumed default, since there's
    /// no honest deadline to have missed.
    pub async fn reap_expired_runs(&self) -> Result<i64, AppError> {
        let now = now_unix();
        self.with_conn(move |conn| {
            let affected = conn.execute(
                "UPDATE runs SET status = 'error', error_class = 'interrupted', \
                 finished_unix = ?1, duration_ms = (?1 - started_unix) * 1000 \
                 WHERE status = 'running' AND deadline_s IS NOT NULL AND started_unix IS NOT NULL \
                 AND (started_unix + deadline_s) < ?1",
                params![now],
            )?;
            Ok(affected as i64)
        })
        .await
    }

    /// P0 requirement 4 / open question: "restart re-leases `running` rows
    /// as `queued` once" -- called once at `mcphost serve` startup, before
    /// the executor starts leasing. Only rows still on their first attempt
    /// (`attempt = 1`) are re-queued; a run that crashed a SECOND time
    /// (already `attempt = 2` from this same function's first pass) is left
    /// `running` so `reap_expired_runs`'s deadline-based rule eventually
    /// catches it as `error: interrupted` instead of being requeued
    /// forever -- "once" per the requirement's own word. Returns the count
    /// requeued.
    pub async fn requeue_interrupted_runs_once(&self) -> Result<i64, AppError> {
        self.with_conn(|conn| {
            let affected = conn.execute(
                "UPDATE runs SET status = 'queued', started_unix = NULL, attempt = attempt + 1 \
                 WHERE status = 'running' AND attempt = 1",
                [],
            )?;
            Ok(affected as i64)
        })
        .await
    }

    /// `host.usage`'s `jobs` block (P0 requirement 7): counts and total
    /// wall-clock seconds of this tenant's `trigger = 'job'` runs finalized
    /// within the window.
    pub async fn jobs_usage(&self, tenant_id: i64, window_secs: i64) -> Result<JobsUsage, AppError> {
        let since = now_unix() - window_secs;
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT status, duration_ms FROM runs WHERE tenant_id = ?1 AND trigger = 'job' \
                 AND finished_unix >= ?2",
            )?;
            let mut usage = JobsUsage::default();
            let rows = stmt.query_map(params![tenant_id, since], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?))
            })?;
            for row in rows {
                let (status, duration_ms) = row?;
                usage.seconds += duration_ms.unwrap_or(0) / 1000;
                match status.as_str() {
                    "done" => usage.done += 1,
                    "error" => usage.error += 1,
                    "timeout" => usage.timeout += 1,
                    "cancelled" => usage.cancelled += 1,
                    _ => {}
                }
            }
            Ok(usage)
        })
        .await
    }

    /// The most recent call's metered `cpu_ms`/`peak_rss_kb` for a tenant's
    /// tool -- test/AC1-only helper (PRD-mcphost-code-tools requirement 8:
    /// "the `calls` row records cpu and memory") to read back what
    /// `record_call` stored without adding a new RPC surface.
    pub async fn last_call_usage(
        &self,
        tenant_id: i64,
        tool_name: String,
    ) -> Result<Option<(Option<i64>, Option<i64>)>, AppError> {
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT cpu_ms, peak_rss_kb FROM calls WHERE tenant_id = ?1 AND tool_name = ?2 \
                 ORDER BY id DESC LIMIT 1",
                params![tenant_id, tool_name],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(AppError::from)
        })
        .await
    }

    /// The most recent call's `outcome` (`ok`/`waited`/`building`/`error`/
    /// `timeout`) for a tenant's tool -- test-only helper (PRD-mcphost-
    /// first-call-reliability requirement 6, AC6) to read back what
    /// `record_call` stored without adding a new RPC surface, mirroring
    /// [`Self::last_call_usage`].
    pub async fn last_call_outcome(
        &self,
        tenant_id: i64,
        tool_name: String,
    ) -> Result<Option<String>, AppError> {
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT outcome FROM calls WHERE tenant_id = ?1 AND tool_name = ?2 \
                 ORDER BY id DESC LIMIT 1",
                params![tenant_id, tool_name],
                |r| r.get(0),
            )
            .optional()
            .map_err(AppError::from)
        })
        .await
    }

    /// PRD-grand-loop-billing `calls_per_day` enforcement: the number of
    /// this tenant's calls at or after `since_unix` (requirement:
    /// "counted from the existing `calls` table", `idx_calls_tenant_started`
    /// already indexes exactly this). `ok_only` counts successful calls
    /// only (the quota this crate enforces, AC3: "500 ok calls"); passing
    /// `false` would count every attempt including rejections, which
    /// nothing here currently needs but is a one-argument distinction
    /// worth keeping explicit rather than silently picking one.
    /// PRD-mcphost-sharing requirement 4 (AC5): a cross-tenant call counts
    /// against the CALLER's `calls_per_day`, not the owner's -- so a row
    /// with `caller_tenant_id` set counts toward that caller's total
    /// instead of the row's own `tenant_id` (the owner, whose sandbox ran
    /// it). A same-tenant call (`caller_tenant_id IS NULL`) counts under
    /// `tenant_id` exactly as before this PRD.
    pub async fn count_calls_since(
        &self,
        tenant_id: i64,
        since_unix: i64,
        ok_only: bool,
    ) -> Result<i64, AppError> {
        self.with_conn(move |conn| {
            let sql = if ok_only {
                "SELECT COUNT(*) FROM calls WHERE started_unix >= ?2 AND ok = 1 \
                 AND ((caller_tenant_id IS NULL AND tenant_id = ?1) OR caller_tenant_id = ?1)"
            } else {
                "SELECT COUNT(*) FROM calls WHERE started_unix >= ?2 \
                 AND ((caller_tenant_id IS NULL AND tenant_id = ?1) OR caller_tenant_id = ?1)"
            };
            conn.query_row(sql, params![tenant_id, since_unix], |r| r.get(0))
                .map_err(AppError::from)
        })
        .await
    }

    pub async fn usage(&self, tenant_id: i64, window_secs: i64) -> Result<UsageStats, AppError> {
        let since = now_unix() - window_secs;
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT duration_ms, ok, error_class FROM calls \
                 WHERE tenant_id = ?1 AND started_unix >= ?2",
            )?;
            let rows = stmt
                .query_map(params![tenant_id, since], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)? != 0,
                        r.get::<_, Option<String>>(2)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            let mut durations: Vec<i64> = rows.iter().map(|(d, _, _)| *d).collect();
            durations.sort_unstable();
            let errors = rows.iter().filter(|(_, ok, _)| !ok).count() as i64;
            // Requirement 7 (AC8): counted from `error_class`, the same
            // column `record_call` stores `AppError::code()` in (a
            // `capacity` refusal writes `error_class = "capacity"` from
            // `handler.rs`'s existing `Ok(Err(kind_err))` branch -- no new
            // column, just a new count over the one already there).
            let capacity_refusals = rows
                .iter()
                .filter(|(_, _, class)| class.as_deref() == Some("capacity"))
                .count() as i64;
            Ok(UsageStats {
                calls: rows.len() as i64,
                errors,
                p50_ms: percentile(&durations, 0.50),
                p95_ms: percentile(&durations, 0.95),
                capacity_refusals,
            })
        })
        .await
    }

    /// Per-tenant, per-tool usage for the admin view.
    pub async fn usage_by_tenant_and_tool(
        &self,
        window_secs: i64,
    ) -> Result<Vec<ToolUsage>, AppError> {
        let since = now_unix() - window_secs;
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT t.namespace, c.tool_name, c.duration_ms, c.ok, c.error_class \
                 FROM calls c JOIN tenants t ON t.id = c.tenant_id \
                 WHERE c.started_unix >= ?1 \
                 ORDER BY t.namespace, c.tool_name",
            )?;
            let rows = stmt
                .query_map(params![since], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, i64>(3)? != 0,
                        r.get::<_, Option<String>>(4)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;

            use std::collections::BTreeMap;
            // Iteration 1 (rustbuild Stage 3, 2026-09-02): clippy's
            // type-complexity threshold was scaffolded tighter (200) than
            // default (250); factor the grouping type out per the lint's
            // own suggestion rather than raise the read-only threshold.
            type CallDurationsByTenantTool =
                BTreeMap<(String, String), Vec<(i64, bool, Option<String>)>>;
            let mut grouped: CallDurationsByTenantTool = BTreeMap::new();
            for (ns, tool, dur, ok, error_class) in rows {
                grouped
                    .entry((ns, tool))
                    .or_default()
                    .push((dur, ok, error_class));
            }
            let mut out = Vec::with_capacity(grouped.len());
            for ((namespace, tool_name), entries) in grouped {
                let mut durations: Vec<i64> = entries.iter().map(|(d, _, _)| *d).collect();
                durations.sort_unstable();
                let errors = entries.iter().filter(|(_, ok, _)| !ok).count() as i64;
                let capacity_refusals = entries
                    .iter()
                    .filter(|(_, _, class)| class.as_deref() == Some("capacity"))
                    .count() as i64;
                out.push(ToolUsage {
                    namespace,
                    tool_name,
                    stats: UsageStats {
                        calls: entries.len() as i64,
                        errors,
                        p50_ms: percentile(&durations, 0.50),
                        p95_ms: percentile(&durations, 0.95),
                        capacity_refusals,
                    },
                });
            }
            Ok(out)
        })
        .await
    }

    // ---- logs ---------------------------------------------------------

    pub async fn append_log(
        &self,
        tenant_id: i64,
        tool_name: String,
        line: String,
    ) -> Result<(), AppError> {
        let ts = now_rfc3339();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO logs (tenant_id, tool_name, ts, line) VALUES (?1, ?2, ?3, ?4)",
                params![tenant_id, tool_name, ts, line],
            )?;
            Ok(())
        })
        .await
    }

    /// The last `limit` log lines for a tool, oldest first.
    pub async fn tail_logs(
        &self,
        tenant_id: i64,
        tool_name: String,
        limit: i64,
    ) -> Result<Vec<String>, AppError> {
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT line FROM logs WHERE tenant_id = ?1 AND tool_name = ?2 \
                 ORDER BY id DESC LIMIT ?3",
            )?;
            let mut rows = stmt
                .query_map(params![tenant_id, tool_name, limit], |r| {
                    r.get::<_, String>(0)
                })?
                .collect::<Result<Vec<_>, _>>()?;
            rows.reverse();
            Ok(rows)
        })
        .await
    }

    // ---- billing ledger (PRD-grand-loop-billing) ------------------------

    /// AC6: idempotency check before `process_webhook` applies anything --
    /// a second delivery of the same `event_id` is a no-op.
    pub async fn billing_event_exists(&self, event_id: String) -> Result<bool, AppError> {
        self.with_conn(move |conn| {
            conn.prepare("SELECT 1 FROM billing_events WHERE event_id = ?1")?
                .exists(params![event_id])
                .map_err(AppError::from)
        })
        .await
    }

    /// Insert one ledger row. Callers check [`Self::billing_event_exists`]
    /// first (so a duplicate delivery is detected and short-circuited
    /// *before* any tenant mutation is attempted, not merely before the
    /// ledger write) -- this uses a plain `INSERT`, not `INSERT OR IGNORE`,
    /// so a UNIQUE-constraint violation here is a real bug (a check that
    /// should have caught it didn't), not a silently-swallowed race.
    pub async fn insert_billing_event(&self, event: BillingEventInsert) -> Result<(), AppError> {
        let received_at = now_rfc3339();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO billing_events \
                 (event_id, event_type, tenant_id, plan, amount_cents, currency, mode, received_at, payload_sha256) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    event.event_id,
                    event.event_type,
                    event.tenant_id,
                    event.plan,
                    event.amount_cents,
                    event.currency,
                    event.mode,
                    received_at,
                    event.payload_sha256,
                ],
            )?;
            Ok(())
        })
        .await
    }

    /// AC9: `admin.billing_ledger(since?, until?, tenant?)` -- newest first,
    /// capped at 1000 rows. `since`/`until` are unix seconds compared
    /// against `received_at`'s own `unix:<secs>.<nanos>` sort-friendly
    /// encoding (see `now_rfc3339`), so the comparison is done on the
    /// numeric column each row's `id` already orders by (monotonic with
    /// insertion time on this single-writer connection) rather than a
    /// string comparison on `received_at` itself.
    pub async fn list_billing_events(
        &self,
        since_unix: Option<i64>,
        until_unix: Option<i64>,
        tenant_namespace: Option<String>,
    ) -> Result<Vec<BillingEventRow>, AppError> {
        self.with_conn(move |conn| {
            let mut sql = String::from(
                "SELECT be.event_id, be.event_type, t.namespace, be.plan, be.amount_cents, \
                        be.currency, be.mode, be.received_at, be.id \
                 FROM billing_events be LEFT JOIN tenants t ON t.id = be.tenant_id \
                 WHERE 1 = 1",
            );
            let mut bind: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
            if let Some(since) = since_unix {
                sql.push_str(" AND CAST(substr(be.received_at, 6) AS REAL) >= ?");
                bind.push(Box::new(since));
            }
            if let Some(until) = until_unix {
                sql.push_str(" AND CAST(substr(be.received_at, 6) AS REAL) <= ?");
                bind.push(Box::new(until));
            }
            if let Some(ns) = tenant_namespace {
                sql.push_str(" AND t.namespace = ?");
                bind.push(Box::new(ns));
            }
            sql.push_str(" ORDER BY be.id DESC LIMIT 1000");

            let params_refs: Vec<&dyn rusqlite::ToSql> =
                bind.iter().map(|b| b.as_ref()).collect();
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map(params_refs.as_slice(), |r| {
                    Ok(BillingEventRow {
                        event_id: r.get(0)?,
                        event_type: r.get(1)?,
                        tenant: r.get(2)?,
                        plan: r.get(3)?,
                        amount_cents: r.get(4)?,
                        currency: r.get(5)?,
                        mode: r.get(6)?,
                        received_at: r.get(7)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    // ---- metered overage (PRD-mcphost-metered-overage) ------------------

    /// AC2: `process_webhook`'s `checkout.session.completed` handler stores
    /// the event's own `customer` id here (never a `subscription` fallback
    /// -- see [`Tenant::stripe_customer_id`]'s doc comment).
    pub async fn set_stripe_customer_id(
        &self,
        tenant_id: i64,
        stripe_customer_id: String,
    ) -> Result<(), AppError> {
        self.with_conn(move |conn| {
            conn.execute(
                "UPDATE tenants SET stripe_customer_id = ?1 WHERE id = ?2",
                params![stripe_customer_id, tenant_id],
            )?;
            Ok(())
        })
        .await
    }

    /// `meter_state`'s single row: `(last_call_id, updated_at)`. The row is
    /// seeded by migration 0007, so this always finds one.
    pub async fn get_meter_state(&self) -> Result<(i64, Option<String>), AppError> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT last_call_id, updated_at FROM meter_state WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(AppError::from)
        })
        .await
    }

    /// Advance the high-water mark. Called only after a batch's POST(s) have
    /// all succeeded (requirement: "advances `last_call_id` only after a
    /// 2xx").
    pub async fn advance_meter_state(&self, last_call_id: i64) -> Result<(), AppError> {
        let updated_at = now_rfc3339();
        self.with_conn(move |conn| {
            conn.execute(
                "UPDATE meter_state SET last_call_id = ?1, updated_at = ?2 WHERE id = 1",
                params![last_call_id, updated_at],
            )?;
            Ok(())
        })
        .await
    }

    /// One pro tenant's unemitted span: every `ok = 1` call above
    /// `after_call_id`, grouped per tenant (requirement: "groups per
    /// tenant"), for a tenant on `plan = 'pro'` with a
    /// `stripe_customer_id` on file (a pro tenant upgraded before this
    /// migration, or via `admin.plan_set`, has none yet and is silently
    /// skipped until the webhook -- or an operator -- backfills one).
    pub async fn pending_meter_groups(&self, after_call_id: i64) -> Result<Vec<MeterGroup>, AppError> {
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT c.tenant_id, t.stripe_customer_id, COUNT(*), MIN(c.id), MAX(c.id) \
                 FROM calls c JOIN tenants t ON t.id = c.tenant_id \
                 WHERE c.ok = 1 AND c.id > ?1 AND t.plan = 'pro' AND t.stripe_customer_id IS NOT NULL \
                 GROUP BY c.tenant_id ORDER BY c.tenant_id",
            )?;
            let rows = stmt
                .query_map(params![after_call_id], |r| {
                    Ok(MeterGroup {
                        tenant_id: r.get(0)?,
                        stripe_customer_id: r.get(1)?,
                        count: r.get(2)?,
                        first_call_id: r.get(3)?,
                        last_call_id: r.get(4)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// `/healthz`'s `meter_lag` (AC8): the total count of pro-tenant ok
    /// calls still above the high-water mark, across every tenant --
    /// [`Self::pending_meter_groups`]'s counts summed, but a single `COUNT`
    /// rather than a per-tenant `GROUP BY` since `/healthz` only needs the
    /// scalar.
    pub async fn meter_lag(&self, after_call_id: i64) -> Result<i64, AppError> {
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM calls c JOIN tenants t ON t.id = c.tenant_id \
                 WHERE c.ok = 1 AND c.id > ?1 AND t.plan = 'pro' AND t.stripe_customer_id IS NOT NULL",
                params![after_call_id],
                |r| r.get(0),
            )
            .map_err(AppError::from)
        })
        .await
    }

    /// Ledger one batch's span, detecting a replay (AC5) by whether a row
    /// already exists for this exact `(tenant_id, first_call_id,
    /// last_call_id)` triple -- the shape a crash between a batch's POST(s)
    /// and [`Self::advance_meter_state`] leaves behind: the ledger row from
    /// the interrupted run is still absent (it's written in the same pass
    /// as the POST, before the crash point in this scenario) *or* present
    /// depending on exactly when the crash landed, so this check is what
    /// makes the rerun's outcome correct either way -- a rerun after the
    /// interrupted run got far enough to ledger its own row now finds that
    /// row and reports `"replay"` instead of double-ledgering a fresh
    /// `"sent"`. Returns the mode actually written.
    // Same targeted #[allow] convention as `record_call` above:
    // clippy.toml's scaffolded too-many-arguments-threshold (5) is tighter
    // than the default (7); the batch/span/count/mode shape here reads
    // more clearly as separate parameters than as a wrapper struct built
    // only to satisfy the lint.
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_meter_event_ledger_detecting_replay(
        &self,
        batch_id: String,
        tenant_id: i64,
        first_call_id: i64,
        last_call_id: i64,
        count: i64,
    ) -> Result<&'static str, AppError> {
        let created_at = now_rfc3339();
        self.with_conn(move |conn| {
            let existed: bool = conn
                .prepare(
                    "SELECT 1 FROM meter_events \
                     WHERE tenant_id = ?1 AND first_call_id = ?2 AND last_call_id = ?3 LIMIT 1",
                )?
                .exists(params![tenant_id, first_call_id, last_call_id])?;
            let mode: &'static str = if existed { "replay" } else { "sent" };
            conn.execute(
                "INSERT INTO meter_events \
                 (batch_id, tenant_id, first_call_id, last_call_id, count, mode, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![batch_id, tenant_id, first_call_id, last_call_id, count, mode, created_at],
            )?;
            Ok(mode)
        })
        .await
    }

    /// The number of ledgered `meter_events` rows, optionally scoped to one
    /// tenant -- used by `tests/metering_ac*.rs` to assert "one ledger row
    /// per batch span" (AC4/AC5) and by the future P1 `admin.meter_status`.
    pub async fn count_meter_events(&self, tenant_id: Option<i64>) -> Result<i64, AppError> {
        self.with_conn(move |conn| {
            match tenant_id {
                Some(id) => conn.query_row(
                    "SELECT COUNT(*) FROM meter_events WHERE tenant_id = ?1",
                    params![id],
                    |r| r.get(0),
                ),
                None => conn.query_row("SELECT COUNT(*) FROM meter_events", [], |r| r.get(0)),
            }
            .map_err(AppError::from)
        })
        .await
    }

    /// P1 `admin.meter_status` / `billing.status`: a tenant's total emitted
    /// call count from ledgered batches since `since_unix`. `created_at` is
    /// an RFC 3339 UTC string (`now_rfc3339()`), which sorts lexicographically
    /// in the same order it sorts chronologically -- comparing it against
    /// `since_unix` rendered the same way avoids pulling in a date-parsing
    /// dependency this crate otherwise has no need for.
    pub async fn emitted_call_count_for_tenant(
        &self,
        tenant_id: i64,
        since_unix: i64,
    ) -> Result<i64, AppError> {
        let since = crate::state::rfc3339_from_unix(since_unix);
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT COALESCE(SUM(count), 0) FROM meter_events \
                 WHERE tenant_id = ?1 AND created_at >= ?2",
                params![tenant_id, since],
                |r| r.get(0),
            )
            .map_err(AppError::from)
        })
        .await
    }

    /// `admin.meter_status` (AC11): every tenant with at least one ledgered
    /// event since `since_unix`, namespace plus summed `count`, newest
    /// emitter first. Same RFC 3339 string-comparison approach as
    /// [`Self::emitted_call_count_for_tenant`], grouped instead of scoped to
    /// one tenant.
    pub async fn monthly_emitted_counts_by_tenant(
        &self,
        since_unix: i64,
    ) -> Result<Vec<(String, i64)>, AppError> {
        let since = crate::state::rfc3339_from_unix(since_unix);
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT t.namespace, SUM(me.count) FROM meter_events me \
                 JOIN tenants t ON t.id = me.tenant_id \
                 WHERE me.created_at >= ?1 \
                 GROUP BY me.tenant_id ORDER BY SUM(me.count) DESC",
            )?;
            let rows = stmt
                .query_map(params![since], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// `admin.meter_status` (AC11): the most recently ledgered batch's span
    /// -- its `batch_id`, the covered `first_call_id..last_call_id` range
    /// across every group that batch sent, the total call count, and when it
    /// landed. `None` when no batch has ever been ledgered. "Most recent" is
    /// the ledger's own insertion order (`rowid`), which matches wall-clock
    /// order since rows are only ever appended, never reordered.
    pub async fn last_meter_batch_span(&self) -> Result<Option<MeterBatchSpan>, AppError> {
        self.with_conn(|conn| {
            let latest_batch_id: Option<String> = conn
                .query_row(
                    "SELECT batch_id FROM meter_events ORDER BY rowid DESC LIMIT 1",
                    [],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(batch_id) = latest_batch_id else {
                return Ok(None);
            };
            let (first_call_id, last_call_id, count, created_at) = conn.query_row(
                "SELECT MIN(first_call_id), MAX(last_call_id), SUM(count), MAX(created_at) \
                 FROM meter_events WHERE batch_id = ?1",
                params![batch_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )?;
            Ok(Some(MeterBatchSpan {
                batch_id,
                first_call_id,
                last_call_id,
                count,
                created_at,
            }))
        })
        .await
    }

    // ---- triggers (PRD-mcphost-schedules) ---------------------------------

    /// `host.usage`'s scheduled-run counterpart to [`Self::jobs_usage`]
    /// (P0 requirement 5): counts and total wall-clock seconds of this
    /// tenant's `trigger = 'schedule'` runs finalized within the window,
    /// `skipped` (AC5's overlap outcome) included as its own bucket since
    /// it's neither a job success nor a job failure.
    pub async fn scheduled_usage(
        &self,
        tenant_id: i64,
        window_secs: i64,
    ) -> Result<ScheduledUsage, AppError> {
        let since = now_unix() - window_secs;
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT status, duration_ms FROM runs WHERE tenant_id = ?1 AND trigger = 'schedule' \
                 AND finished_unix >= ?2",
            )?;
            let mut usage = ScheduledUsage::default();
            let rows = stmt.query_map(params![tenant_id, since], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?))
            })?;
            for row in rows {
                let (status, duration_ms) = row?;
                usage.seconds += duration_ms.unwrap_or(0) / 1000;
                match status.as_str() {
                    "done" => usage.done += 1,
                    "error" => usage.error += 1,
                    "timeout" => usage.timeout += 1,
                    "cancelled" => usage.cancelled += 1,
                    "skipped" => usage.skipped += 1,
                    _ => {}
                }
            }
            Ok(usage)
        })
        .await
    }

    /// P0 requirement 1: inserts a new `triggers` row (`enabled = 1`,
    /// `created_unix = now`). A duplicate `(tenant_id, tool_name, kind,
    /// config_hash)` fails the table's own `UNIQUE` constraint, surfaced by
    /// `rusqlite`'s ordinary `Err` -- `triggers::set` maps that into a
    /// `trigger_invalid` naming `schedule` (the config, not the id, is what
    /// collided).
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_trigger(
        &self,
        id: String,
        tenant_id: i64,
        tool_name: String,
        kind: String,
        config_json: String,
        config_hash: String,
        next_unix: Option<i64>,
    ) -> Result<(), AppError> {
        let now = now_unix();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO triggers (id, tenant_id, tool_name, kind, config_json, config_hash, \
                 enabled, created_unix, next_unix) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?8)",
                params![id, tenant_id, tool_name, kind, config_json, config_hash, now, next_unix],
            )?;
            Ok(())
        })
        .await
    }

    /// `host.trigger.get`/pause/resume/remove's own-tenant lookup.
    pub async fn get_trigger(
        &self,
        tenant_id: i64,
        id: String,
    ) -> Result<Option<TriggerRow>, AppError> {
        self.with_conn(move |conn| {
            conn.query_row(
                &format!("SELECT {TRIGGER_COLUMNS} FROM triggers WHERE id = ?1 AND tenant_id = ?2"),
                params![id, tenant_id],
                trigger_row_from_row,
            )
            .optional()
            .map_err(AppError::from)
        })
        .await
    }

    /// `host.trigger.list(tool?)` -- this tenant's own triggers, newest
    /// first.
    pub async fn list_triggers(
        &self,
        tenant_id: i64,
        tool_name: Option<String>,
    ) -> Result<Vec<TriggerRow>, AppError> {
        self.with_conn(move |conn| {
            let rows = if let Some(tool_name) = tool_name {
                let mut stmt = conn.prepare(&format!(
                    "SELECT {TRIGGER_COLUMNS} FROM triggers WHERE tenant_id = ?1 AND tool_name = ?2 \
                     ORDER BY rowid DESC"
                ))?;
                stmt.query_map(params![tenant_id, tool_name], trigger_row_from_row)?
                    .collect::<Result<Vec<_>, _>>()?
            } else {
                let mut stmt = conn.prepare(&format!(
                    "SELECT {TRIGGER_COLUMNS} FROM triggers WHERE tenant_id = ?1 ORDER BY rowid DESC"
                ))?;
                stmt.query_map(params![tenant_id], trigger_row_from_row)?
                    .collect::<Result<Vec<_>, _>>()?
            };
            Ok(rows)
        })
        .await
    }

    /// P0 requirement 4's quota check: how many `kind = 'schedule'`
    /// triggers this tenant holds right now, paused or not -- only
    /// `host.trigger.remove` frees a slot (pausing doesn't), so a paused
    /// trigger still counts.
    pub async fn count_schedule_triggers_for_tenant(&self, tenant_id: i64) -> Result<i64, AppError> {
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM triggers WHERE tenant_id = ?1 AND kind = 'schedule'",
                params![tenant_id],
                |r| r.get(0),
            )
            .map_err(AppError::from)
        })
        .await
    }

    /// `host.trigger.pause`/`resume`. Returns `true` if a row for this
    /// tenant/id was actually updated (a caller reaching for someone
    /// else's id, or an id that never existed, both read as
    /// `trigger_not_found` rather than a silent no-op).
    pub async fn set_trigger_enabled(
        &self,
        tenant_id: i64,
        id: String,
        enabled: bool,
    ) -> Result<bool, AppError> {
        self.with_conn(move |conn| {
            let affected = conn.execute(
                "UPDATE triggers SET enabled = ?1 WHERE id = ?2 AND tenant_id = ?3",
                params![enabled, id, tenant_id],
            )?;
            Ok(affected > 0)
        })
        .await
    }

    /// `host.trigger.remove`. Returns `true` if a row was actually deleted.
    pub async fn remove_trigger(&self, tenant_id: i64, id: String) -> Result<bool, AppError> {
        self.with_conn(move |conn| {
            let affected = conn.execute(
                "DELETE FROM triggers WHERE id = ?1 AND tenant_id = ?2",
                params![id, tenant_id],
            )?;
            Ok(affected > 0)
        })
        .await
    }

    /// P0 requirement 1: `host.tool_remove` disables (never deletes --
    /// `host.trigger.list` should still show a removed tool's old
    /// schedules as evidence, not silently vanish them) every trigger on
    /// `tool_name`, reporting the count for the tool-remove result's own
    /// `triggers_disabled`.
    pub async fn disable_triggers_for_tool(
        &self,
        tenant_id: i64,
        tool_name: String,
    ) -> Result<i64, AppError> {
        self.with_conn(move |conn| {
            let affected = conn.execute(
                "UPDATE triggers SET enabled = 0 \
                 WHERE tenant_id = ?1 AND tool_name = ?2 AND enabled = 1",
                params![tenant_id, tool_name],
            )?;
            Ok(affected as i64)
        })
        .await
    }

    /// P0 requirement 3: the scheduler tick's own due-list scan --
    /// cross-tenant, `enabled` schedule triggers whose `next_unix` has
    /// passed. `ORDER BY next_unix` so, on a tick catching up after
    /// downtime, the longest-overdue schedules enqueue first.
    pub async fn due_schedule_triggers(&self, now: i64) -> Result<Vec<TriggerRow>, AppError> {
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {TRIGGER_COLUMNS} FROM triggers \
                 WHERE kind = 'schedule' AND enabled = 1 AND next_unix IS NOT NULL AND next_unix <= ?1 \
                 ORDER BY next_unix"
            ))?;
            let rows = stmt
                .query_map(params![now], trigger_row_from_row)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// The scheduler tick's own most-recent-run lookup for a trigger
    /// (P0 requirement 3's overlap check): `None` if this trigger has never
    /// fired.
    pub async fn last_run_status_for_trigger(
        &self,
        trigger_ref: String,
    ) -> Result<Option<String>, AppError> {
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT status FROM runs WHERE trigger_ref = ?1 ORDER BY rowid DESC LIMIT 1",
                params![trigger_ref],
                |r| r.get(0),
            )
            .optional()
            .map_err(AppError::from)
        })
        .await
    }

    /// After every firing (enqueued or skipped): advances `next_unix` to
    /// the following occurrence and stamps `last_fired_unix`;
    /// `last_run_id` updates only for an actual enqueue (`None` leaves the
    /// column at whatever it already held, so a skipped firing doesn't
    /// clobber the last real run's id).
    pub async fn update_trigger_after_fire(
        &self,
        id: String,
        next_unix: Option<i64>,
        last_run_id: Option<String>,
        last_fired_unix: i64,
    ) -> Result<(), AppError> {
        self.with_conn(move |conn| {
            if let Some(last_run_id) = last_run_id {
                conn.execute(
                    "UPDATE triggers SET next_unix = ?1, last_run_id = ?2, last_fired_unix = ?3 \
                     WHERE id = ?4",
                    params![next_unix, last_run_id, last_fired_unix, id],
                )?;
            } else {
                conn.execute(
                    "UPDATE triggers SET next_unix = ?1, last_fired_unix = ?2 WHERE id = ?3",
                    params![next_unix, last_fired_unix, id],
                )?;
            }
            Ok(())
        })
        .await
    }

    /// `admin.triggers(tenant?, kind?)`.
    pub async fn admin_list_triggers(
        &self,
        tenant_id: Option<i64>,
        kind: Option<String>,
    ) -> Result<Vec<TriggerRow>, AppError> {
        self.with_conn(move |conn| {
            let mut sql = format!("SELECT {TRIGGER_COLUMNS} FROM triggers WHERE 1=1");
            let mut idx = 1;
            let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
            if let Some(t) = tenant_id {
                sql.push_str(&format!(" AND tenant_id = ?{idx}"));
                binds.push(Box::new(t));
                idx += 1;
            }
            if let Some(k) = kind {
                sql.push_str(&format!(" AND kind = ?{idx}"));
                binds.push(Box::new(k));
                idx += 1;
            }
            let _ = idx;
            sql.push_str(" ORDER BY rowid DESC");
            let refs: Vec<&dyn rusqlite::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map(refs.as_slice(), trigger_row_from_row)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// `/healthz`'s `schedules_enabled` (P0 requirement 5): host-wide count
    /// of enabled `kind = 'schedule'` triggers, across every tenant.
    pub async fn count_enabled_schedule_triggers(&self) -> Result<i64, AppError> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM triggers WHERE kind = 'schedule' AND enabled = 1",
                [],
                |r| r.get(0),
            )
            .map_err(AppError::from)
        })
        .await
    }
}

/// [`Db::last_meter_batch_span`]'s return shape -- one ledgered batch's
/// covered span across every tenant group it sent.
#[derive(Debug, Clone, Serialize)]
pub struct MeterBatchSpan {
    pub batch_id: String,
    pub first_call_id: i64,
    pub last_call_id: i64,
    pub count: i64,
    pub created_at: String,
}

/// One pro tenant's pending (unemitted) span, as
/// [`Db::pending_meter_groups`] reads it back.
#[derive(Debug, Clone, Serialize)]
pub struct MeterGroup {
    pub tenant_id: i64,
    pub stripe_customer_id: String,
    pub count: i64,
    pub first_call_id: i64,
    pub last_call_id: i64,
}
