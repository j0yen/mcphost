//! SQLite storage. One dedicated blocking task services every query
//! (`spawn_blocking` around a mutex-guarded [`rusqlite::Connection`]) so a
//! slow query never stalls the async runtime, per the PRD's technical
//! considerations.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::errors::AppError;

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

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
const MIGRATION_0017: &str = include_str!("../migrations/0017_event_dedupe.sql");
const MIGRATION_0018: &str = include_str!("../migrations/0018_runs_test_run.sql");
const MIGRATION_0019: &str = include_str!("../migrations/0019_handoff_token.sql");
const MIGRATION_0020: &str = include_str!("../migrations/0020_agent_profiles.sql");
const MIGRATION_0021: &str = include_str!("../migrations/0021_messaging.sql");
const MIGRATION_0022: &str = include_str!("../migrations/0022_message_refused.sql");
const MIGRATION_0023: &str = include_str!("../migrations/0023_message_triggers.sql");
const MIGRATION_0024: &str = include_str!("../migrations/0024_consent.sql");
const MIGRATION_0025: &str = include_str!("../migrations/0025_self_offboard.sql");
const MIGRATION_0026: &str = include_str!("../migrations/0026_retention.sql");
const MIGRATION_0027: &str = include_str!("../migrations/0027_mesh_ops.sql");
const MIGRATION_0028: &str = include_str!("../migrations/0028_tool_versions.sql");
const MIGRATION_0029: &str = include_str!("../migrations/0029_agent_channels.sql");
const MIGRATION_0030: &str = include_str!("../migrations/0030_tool_lock.sql");

/// PRD-mcphost-inbound-events P1 requirement 7 / AC11: "a repeated id
/// *within 24 h*" -- [`Db::claim_event_dedupe`]'s own freshness window.
const EVENT_DEDUPE_WINDOW_S: i64 = 86_400;

/// PRD-mcphost-agent-inbox requirement 8 / AC4: same 24h freshness window
/// as [`EVENT_DEDUPE_WINDOW_S`], for `host.msg.send`'s `dedupe_key`.
const MSG_DEDUPE_WINDOW_MS: i64 = 86_400_000;

/// Shared by every query that selects a whole tenant row, so the column
/// list and [`tenant_from_row`] stay in lockstep with each other.
const TENANT_COLUMNS: &str = "id, namespace, display_name, key_hash, created_at, disabled, \
    last_tool_change_unix, namespace_verified, registry_namespace, plan, plan_since, billing_ref, \
    stripe_customer_id, synthetic, source_class, client_name, client_version, classified_by, \
    created_unix, origin, origin_detail, key_rotated_unix, disabled_reason, mesh_frozen_at";

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
        key_rotated_unix: r.get(21)?,
        disabled_reason: r.get(22)?,
        mesh_frozen_at: r.get(23)?,
    })
}

/// Shared by every query that selects a whole tools row, so the column
/// list and [`tool_from_row`] stay in lockstep with each other -- same
/// convention as [`TENANT_COLUMNS`]/[`tenant_from_row`] above
/// (PRD-mcphost-sharing migration 0013).
const TOOL_COLUMNS: &str = "id, tenant_id, name, kind, spec, created_at, visibility, \
    share_description, shared_unix, shared_group, unshared_by, current_version";

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
        current_version: r.get(11)?,
    })
}

/// Shared by [`Db::list_tool_versions`]/[`Db::get_tool_version`]'s
/// `SELECT version, kind, spec, created_at, created_unix, source_sha256`.
fn tool_version_from_row(r: &Row) -> rusqlite::Result<ToolVersionRow> {
    let spec_text: String = r.get(2)?;
    Ok(ToolVersionRow {
        version: r.get(0)?,
        kind: r.get(1)?,
        spec: serde_json::from_str(&spec_text).unwrap_or(Value::Null),
        created_at: r.get(3)?,
        created_unix: r.get(4)?,
        source_sha256: r.get(5)?,
    })
}

/// PRD-mcphost-agent-directory requirements 1/4/5: `tenants` LEFT JOINed
/// onto its (possibly absent) `agent_profiles` row -- shared by
/// `Db::lookup_agent`/`Db::lookup_agent_admin`/`Db::list_agent_cards` so the
/// column order and [`agent_card_from_row`] stay in lockstep, same
/// convention as [`TENANT_COLUMNS`]/[`tenant_from_row`] above. Excludes
/// `t.disabled` (each caller filters or not, per requirement 4 vs the
/// admin-lookup goal) and every billing/key field (requirement 2's "never
/// returns" promise, restated for the read side).
const AGENT_CARD_SELECT: &str = "SELECT t.namespace, t.display_name, t.last_seen_unix, \
    t.source_class, t.synthetic, ap.handle, ap.description, ap.tags_json, ap.contact_policy \
    FROM tenants t LEFT JOIN agent_profiles ap ON ap.tenant_id = t.id";

/// The card `host.agent.lookup`/`host.agent.search`/`admin.agent.lookup`
/// return -- never a `key_hash`, `billing_ref`, or `stripe_customer_id`.
#[derive(Debug, Clone, Serialize)]
pub struct AgentCard {
    pub address: String,
    pub handle: Option<String>,
    pub display_name: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub contact_policy: String,
    /// Unix seconds, rounded to the minute (requirement 6); `None` for a
    /// tenant that has never made an authenticated call since this column
    /// was added.
    pub last_seen: Option<i64>,
    pub source_class: Option<String>,
    pub synthetic: bool,
}

fn agent_card_from_row(r: &Row) -> rusqlite::Result<AgentCard> {
    let last_seen_unix: Option<i64> = r.get(2)?;
    let tags_json: Option<String> = r.get(7)?;
    let tags = tags_json
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default();
    Ok(AgentCard {
        address: r.get(0)?,
        display_name: r.get(1)?,
        last_seen: last_seen_unix.map(|u| (u / 60) * 60),
        source_class: r.get(3)?,
        synthetic: r.get::<_, Option<String>>(4)?.is_some(),
        handle: r.get(5)?,
        description: r.get(6)?,
        tags,
        contact_policy: r.get::<_, Option<String>>(8)?.unwrap_or_else(|| "open".to_string()),
    })
}

/// Row shape shared by [`Db::msg_inbox`]/[`Db::msg_thread`]'s
/// `SELECT ... FROM messages m ...` (column order pinned in both call
/// sites' SQL text).
fn message_row_from_row(r: &Row) -> rusqlite::Result<MessageRow> {
    let data_json: Option<String> = r.get(5)?;
    Ok(MessageRow {
        id: r.get(0)?,
        thread_id: r.get(1)?,
        seq: r.get(2)?,
        from_address: r.get(3)?,
        body: r.get(4)?,
        data: data_json.and_then(|s| serde_json::from_str(&s).ok()),
        in_reply_to: r.get(6)?,
        synthetic: r.get(7)?,
        source_class: r.get(8)?,
        created_at: r.get(9)?,
        created_unix_ms: r.get(10)?,
        read_at: r.get(11)?,
        // PRD-mcphost-agent-consent requirement 6: appended as the last
        // column by both `msg_inbox`'s and `msg_thread`'s SELECTs, so every
        // index above stays unchanged.
        urgent: r.get::<_, i64>(12)? != 0,
    })
}

/// The `agent_profiles` row alone (no tenant join) -- what
/// `host.agent.whoami`/`host.agent.profile_set` need, since both already
/// have the caller's [`Tenant`] in hand.
#[derive(Debug, Clone, Default)]
pub struct AgentProfileRow {
    pub handle: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub contact_policy: String,
}

fn agent_profile_row_from_row(r: &Row) -> rusqlite::Result<AgentProfileRow> {
    let tags_json: String = r.get(2)?;
    Ok(AgentProfileRow {
        handle: r.get(0)?,
        description: r.get(1)?,
        tags: serde_json::from_str(&tags_json).unwrap_or_default(),
        contact_policy: r.get(3)?,
    })
}

/// Outcome of [`Db::set_agent_profile`]'s handle-uniqueness check
/// (requirement 3 / AC3): `HandleTaken` never names the current holder.
pub enum SetProfileOutcome {
    Ok(AgentProfileRow),
    HandleTaken,
}

// ---- consent (PRD-mcphost-agent-consent) --------------------------------

/// One `host.agent.contacts()` accepted-pair row.
#[derive(Debug, Clone, Serialize)]
pub struct ContactRow {
    pub address: String,
    pub accepted_at: String,
}

/// One `host.agent.contacts()` `incoming`/`outgoing` request row --
/// `address` is the *other* party (the requester for `incoming`, the
/// target for `outgoing`), never the caller's own.
#[derive(Debug, Clone, Serialize)]
pub struct ContactRequestRow {
    pub request_id: String,
    pub address: String,
    pub note: Option<String>,
    pub status: String,
    pub created_at: String,
    pub decided_at: Option<String>,
}

fn contact_request_row_from_row(r: &Row) -> rusqlite::Result<ContactRequestRow> {
    Ok(ContactRequestRow {
        request_id: r.get(0)?,
        address: r.get(1)?,
        note: r.get(2)?,
        status: r.get(3)?,
        created_at: r.get(4)?,
        decided_at: r.get(5)?,
    })
}

/// [`Db::agent_contacts`]'s whole return shape.
pub struct ContactsView {
    pub contacts: Vec<ContactRow>,
    pub incoming: Vec<ContactRequestRow>,
    pub outgoing: Vec<ContactRequestRow>,
}

/// [`Db::contact_accept`]/[`Db::contact_deny`]'s return shape: the other
/// party's address (always the requester -- both tools are only ever
/// called by the recipient) and, for accept, when the pair became mutual.
pub struct ContactDecision {
    pub request_id: String,
    pub other_address: String,
    pub decided_at: String,
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
    /// PRD-mcphost-handoff-token requirement 3 / P1 requirement 7: unix
    /// seconds of this tenant's last `host.key_rotate`, or `None` for a
    /// tenant that has never rotated (migration 0019). `host.whoami`'s
    /// `key_age_s` is measured from this when set, else from `created_unix`.
    pub key_rotated_unix: Option<i64>,
    /// PRD-mcphost-tenant-self-offboard P1 requirement 5 / AC5: why
    /// `disabled` is true -- `"self_offboard"` for `host.self_offboard()`,
    /// `"admin_disable"` for `admin.tenant_disable`, or `None` for an
    /// enabled tenant (or a row disabled before this column existed --
    /// migration 0025 backfills nothing, since there is no prior reason to
    /// recover). See migration 0025.
    pub disabled_reason: Option<String>,
    /// PRD-mcphost-agent-mesh-ops requirement 4: set (RFC3339) by
    /// `admin.mesh.freeze`, cleared by `admin.mesh.unfreeze`. Checked
    /// directly off this field at the top of `host.msg.send`/`reply`,
    /// `host.channel.post` and `Db::contact_request` -- never consulted by
    /// a read, an ack, `host.msg.wait`, inbound delivery, a trigger, or any
    /// non-messaging tool (migration 0026).
    pub mesh_frozen_at: Option<String>,
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
    /// PRD-mcphost-tool-versions migration 0028: which `tool_versions.version`
    /// is this tool's active one -- `tools.spec`/`tools.kind` always mirror
    /// that version's own content, kept in sync by
    /// [`Db::upsert_tool`]/[`Db::rollback_tool_version`], so an unpinned
    /// call path (`Kind::call(&row.spec, ...)`) needs no change at all.
    pub current_version: i64,
}

/// A single immutable published version of a tool (PRD-mcphost-tool-versions
/// requirement 1/3): what `host.tool_history`/`host.tool_diff` read, and
/// what a version-pinned `host.tool_call`/cross-tenant call dispatches
/// against instead of `tools.spec`.
#[derive(Debug, Clone, Serialize)]
pub struct ToolVersionRow {
    pub version: i64,
    pub kind: String,
    pub spec: Value,
    pub created_at: String,
    pub created_unix: i64,
    pub source_sha256: String,
}

/// [`Db::rollback_tool_version`]'s outcome (requirement 4 / AC2, AC4):
/// distinguishing "no such tool" from "tool exists, version doesn't" is
/// what lets `control::tool_rollback` return `tool_not_found` vs an
/// argument error naming the valid range.
pub enum RollbackOutcome {
    NotFound,
    OutOfRange { min: i64, max: i64 },
    Ok { kind: String },
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

/// PRD-mcphost-tenant-data-export P0 requirement 3 / AC3: the outcome of
/// [`Db::start_export_run`] -- either a fresh `running` row was inserted, or
/// this tenant already had one running and its id is returned instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartExportRun {
    Started(String),
    AlreadyRunning(String),
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
    /// PRD-mcphost-inbound-events P0 requirement 3 / migration 0018 / AC7:
    /// `true` only for a run `host.trigger.test` created (never for a real
    /// `POST /hooks/...` delivery, a replay, or any pre-existing trigger
    /// kind), same additive-column shape as [`Self::manual`].
    pub test: bool,
    /// PRD-mcphost-agent-wake P0 requirement 2 / migration 0023: the
    /// `message_id` a `trigger = "message"` run was fired for -- `None` for
    /// every other trigger kind (and for a `host.trigger.test` dry run on a
    /// message trigger, whose envelope is synthetic, never a real stored
    /// message).
    pub message_id: Option<String>,
}

const RUN_COLUMNS: &str = "id, tenant_id, tool_name, trigger, trigger_ref, caller_tenant_id, \
    status, progress_json, result_ref, error_class, started_unix, finished_unix, duration_ms, \
    deadline_s, attempt, purged_unix, args_json, manual, test_run, message_id";

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
        test: r.get::<_, i64>(18)? != 0,
        message_id: r.get(19)?,
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

/// PRD-mcphost-handoff-token requirement 2 / AC2: [`Db::redeem_handoff_token`]'s
/// outcome. `token_id` (the row's own integer id, never the token or key
/// value) is carried on every variant so a caller can journal a
/// non-`Redeemed` outcome the same values-free way as a successful one.
#[derive(Debug, Clone)]
pub enum HandoffRedeemOutcome {
    Redeemed {
        token_id: i64,
        tenant_id: i64,
        key_enc: Vec<u8>,
        key_nonce: Vec<u8>,
    },
    NotFound,
    Expired {
        token_id: i64,
    },
    AlreadyRedeemed {
        token_id: i64,
    },
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

/// PRD-mcphost-tool-versions migration 0028 backfill: every pre-existing
/// `tools` row becomes its own version 1 (Migration/compatibility section),
/// `source_sha256` computed for real since it can't come from a column
/// default. `INSERT OR IGNORE` makes a second run of a migration that
/// somehow re-executes this (it shouldn't -- gated by
/// `migrate_0028_tool_versions`'s own column check) a no-op rather than a
/// UNIQUE-constraint error.
fn backfill_tool_versions_sync(conn: &Connection) -> Result<(), AppError> {
    let rows: Vec<(i64, String, String, String, String)> = {
        let mut stmt = conn.prepare("SELECT tenant_id, name, kind, spec, created_at FROM tools")?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let created_unix = now_unix();
    for (tenant_id, name, kind, spec, created_at) in rows {
        let source_sha256 = sha256_hex(spec.as_bytes());
        conn.execute(
            "INSERT OR IGNORE INTO tool_versions \
             (tenant_id, name, version, kind, spec, created_at, created_unix, source_sha256) \
             VALUES (?1, ?2, 1, ?3, ?4, ?5, ?6, ?7)",
            params![tenant_id, name, kind, spec, created_at, created_unix, source_sha256],
        )?;
    }
    Ok(())
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

// ---- messaging (PRD-mcphost-agent-inbox) --------------------------------

/// A resolved `host.msg.send`/`reply` recipient or the reason it was
/// refused (requirement 2's four refusal codes, collapsed to what
/// [`Db::msg_send`] actually needs: either a live, addressable tenant or a
/// `&'static str` refusal code to echo back in `refused[].code`). `Tenant`
/// is boxed so this enum doesn't inflate to `Tenant`'s own size on the
/// `Refused` arm (clippy::large_enum_variant).
pub enum RecipientResolution {
    Ok(Box<Tenant>),
    Refused(&'static str),
}

/// The inverse of [`RecipientResolution::Refused`]'s `&'static str`:
/// `messages.refused_json` round-trips as owned `String`s (advisory finding
/// `dedupe-resend-refused-list-not-reconstructed`), so a dedupe-hit
/// rebuild needs to map each stored code back to the same interned static
/// every fresh refusal already uses. Every code this crate ever writes
/// into `refused_json` is one of these three (the fourth refusal code,
/// `quota_exceeded`, only ever fails the whole call -- see `insert_message`
/// and `dedupe_hit`'s doc comments -- so it never appears in a stored
/// per-recipient `refused` list); an unrecognized value (there should
/// never be one) falls back to `agent_not_found` rather than panicking.
fn code_to_static(code: &str) -> &'static str {
    match code {
        "contact_refused" => "contact_refused",
        "recipient_inbox_full" => "recipient_inbox_full",
        // PRD-mcphost-agent-consent requirement 4: the new per-recipient
        // refusal code `consent::check` can return alongside the two
        // inbox-PRD ones above.
        "contact_pending" => "contact_pending",
        _ => "agent_not_found",
    }
}

/// One row of `host.msg.send`'s `{message_id, thread_id, seq, delivered_to,
/// refused}` response (requirement 2) -- built by [`Db::send_message`],
/// turned into wire JSON by `messaging.rs` (this module stays free of
/// `serde_json::Value` shaping, same split every other `db.rs` DTO uses).
pub struct SendOutcome {
    pub message_id: String,
    pub thread_id: String,
    pub seq: i64,
    pub delivered_to: Vec<String>,
    pub refused: Vec<(String, &'static str)>,
    /// `true` when this call reused an existing `(sender, dedupe_key)`
    /// message rather than storing a new one (requirement 8 / AC4) --
    /// `messaging.rs` needs this only to skip the quota-usage bump on a
    /// dedupe hit, never surfaced on the wire.
    pub deduped: bool,
    /// PRD-mcphost-agent-wake requirement 2: the same recipients as
    /// `delivered_to`, by tenant id rather than namespace, same order --
    /// `messaging::fire_message_triggers`' own per-recipient trigger
    /// lookup needs the id, not the address (never surfaced on the wire,
    /// same posture as `deduped`).
    pub delivered_tenant_ids: Vec<i64>,
    /// PRD-mcphost-agent-wake requirement 2: this message's own stored
    /// `created_at` (RFC3339), for the envelope `fire_message_triggers`
    /// builds -- exact, not a fresh "now" taken after this call returns
    /// (never surfaced on the wire, same posture as `deduped`).
    pub created_at: String,
}

/// One `host.msg.inbox`/`host.msg.thread` row (requirement 4).
#[derive(Debug, Clone, Serialize)]
pub struct MessageRow {
    pub id: String,
    pub thread_id: String,
    pub seq: i64,
    pub from_address: String,
    pub body: String,
    pub data: Option<Value>,
    pub in_reply_to: Option<String>,
    pub synthetic: Option<String>,
    pub source_class: String,
    pub created_at: String,
    pub created_unix_ms: i64,
    pub read_at: Option<String>,
    /// PRD-mcphost-agent-consent requirement 6: `true` for a `host.msg.send(
    /// urgent: true)` message -- bypasses `Db::msg_inbox`'s `unread_only`
    /// mute filter, never a block or `closed` policy.
    pub urgent: bool,
}

/// PRD-mcphost-agent-wake requirement 4 / AC4: params for
/// [`Db::insert_rejected_run`], bundled (rather than seven positional
/// arguments) to keep `trigger`/`trigger_ref`/`message_id` -- three
/// adjacent `String`-typed fields -- transposition-proof at the call site.
#[derive(Debug, Clone)]
pub struct RejectedRun {
    pub run_id: String,
    pub tenant_id: i64,
    pub tool_name: String,
    pub trigger: String,
    pub trigger_ref: String,
    pub message_id: Option<String>,
    pub error_class: &'static str,
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
        // PRD-mcphost-data-retention requirement 2 / technical
        // considerations: the nightly prune runs its batched deletes on a
        // second, dedicated connection to this same file (see
        // `retention::prune_sync`) rather than this shared one, so the two
        // can genuinely contend for SQLite's single write lock -- without
        // a `busy_timeout` on THIS connection too, a prune batch holding
        // the write lock would make an ordinary `host.tool_call` insert
        // fail immediately with `database is locked` instead of waiting
        // it out. Same 5s value `tables.rs`'s own per-tenant connections
        // already use for this.
        conn.busy_timeout(crate::retention::PRUNE_BUSY_TIMEOUT)
            .map_err(AppError::from)?;
        let db = Db {
            conn: Arc::new(Mutex::new(conn)),
            path,
        };
        db.migrate_sync()?;
        Ok(db)
    }

    /// PRD-mcphost-tenant-tables: the directory `mcphost.db` itself lives
    /// in -- `tables.rs` derives its own `tables/<tenant_id>.db` per-tenant
    /// files from this rather than a second `MCPHOST_DATA_DIR` read, so the
    /// two storage engines (the shared control-plane db, and each tenant's
    /// own real-SQL table file) always agree on which data dir they're
    /// under, including in tests that point `Db::open` at a scratch dir.
    pub fn data_dir(&self) -> &Path {
        self.path.parent().unwrap_or_else(|| Path::new("."))
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
        Self::migrate_0016_runs_manual(&conn)?;
        Self::migrate_0017_event_dedupe(&conn)?;
        Self::migrate_0018_runs_test_run(&conn)?;
        Self::migrate_0019_handoff_token(&conn)?;
        Self::migrate_0020_agent_profiles(&conn)?;
        Self::migrate_0021_messaging(&conn)?;
        Self::migrate_0022_message_refused(&conn)?;
        Self::migrate_0023_message_triggers(&conn)?;
        Self::migrate_0024_consent(&conn)?;
        Self::migrate_0025_self_offboard(&conn)?;
        Self::migrate_0026_retention(&conn)?;
        Self::migrate_0027_mesh_ops(&conn)?;
        Self::migrate_0028_tool_versions(&conn)?;
        Self::migrate_0029_agent_channels(&conn)?;
        Self::migrate_0030_tool_lock(&conn)
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

    /// PRD-mcphost-inbound-events P1 requirement 7: same new-table
    /// idempotency guard as 0011/0014/0015 above.
    fn migrate_0017_event_dedupe(conn: &Connection) -> Result<(), AppError> {
        let has_table: bool = conn
            .prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'event_dedupe'")?
            .exists([])?;
        if !has_table {
            conn.execute_batch(MIGRATION_0017)?;
        }
        Ok(())
    }

    /// PRD-mcphost-inbound-events P0 requirement 3: `ALTER TABLE ADD COLUMN`
    /// has no `IF NOT EXISTS`, same idempotency guard 0002/0013/0016 above
    /// use for their own added columns.
    fn migrate_0018_runs_test_run(conn: &Connection) -> Result<(), AppError> {
        let has_column: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('runs') WHERE name = 'test_run'")?
            .exists([])?;
        if !has_column {
            conn.execute_batch(MIGRATION_0018)?;
        }
        Ok(())
    }

    /// PRD-mcphost-handoff-token requirements 1-3: same idempotency pattern
    /// as 0011/0014/0015 (a wholly new, `CREATE TABLE IF NOT EXISTS`-guarded
    /// table) plus 0002/0013/0016's `ALTER TABLE ADD COLUMN` pattern for
    /// `tenants.key_rotated_unix` -- both land in the same batch, gated on
    /// the column (0008's precedent: two additive changes, one gate,
    /// applied atomically the first time this runs).
    fn migrate_0019_handoff_token(conn: &Connection) -> Result<(), AppError> {
        let has_column: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('tenants') WHERE name = 'key_rotated_unix'")?
            .exists([])?;
        if !has_column {
            conn.execute_batch(MIGRATION_0019)?;
        }
        Ok(())
    }

    /// PRD-mcphost-agent-directory requirements 1/6: same shape as 0019
    /// above -- a wholly new table (`agent_profiles`) plus one additive
    /// column (`tenants.last_seen_unix`), gated on the column so a second
    /// `migrate()` call (every `serve` start) is a no-op.
    fn migrate_0020_agent_profiles(conn: &Connection) -> Result<(), AppError> {
        let has_column: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('tenants') WHERE name = 'last_seen_unix'")?
            .exists([])?;
        if !has_column {
            conn.execute_batch(MIGRATION_0020)?;
        }
        Ok(())
    }

    /// PRD-mcphost-agent-inbox requirement 1: same new-table idempotency
    /// guard as 0011/0014/0015/0017/0019/0020 above, gated on `threads`
    /// (the first of the five tables this migration adds).
    fn migrate_0021_messaging(conn: &Connection) -> Result<(), AppError> {
        let has_table: bool = conn
            .prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'threads'")?
            .exists([])?;
        if !has_table {
            conn.execute_batch(MIGRATION_0021)?;
        }
        Ok(())
    }

    /// Advisory finding `dedupe-resend-refused-list-not-reconstructed`
    /// (PRD-mcphost-agent-inbox gate review): additive column, same
    /// `pragma_table_info` idempotency guard as 0002/0020's own
    /// `ALTER TABLE ADD COLUMN` migrations (SQLite has no
    /// `IF NOT EXISTS` for that statement).
    fn migrate_0022_message_refused(conn: &Connection) -> Result<(), AppError> {
        let has_column: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('messages') WHERE name = 'refused_json'")?
            .exists([])?;
        if !has_column {
            conn.execute_batch(MIGRATION_0022)?;
        }
        Ok(())
    }

    /// PRD-mcphost-agent-wake P0 requirements 1-2: same additive-column
    /// idempotency guard as 0016/0018/0022's own `ALTER TABLE ADD COLUMN`
    /// migrations -- gated on `runs.message_id` (the index alongside it is
    /// already `CREATE INDEX IF NOT EXISTS`, safe to re-run unconditionally,
    /// but kept in the same gated batch so both land atomically the first
    /// time this runs).
    fn migrate_0023_message_triggers(conn: &Connection) -> Result<(), AppError> {
        let has_column: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('runs') WHERE name = 'message_id'")?
            .exists([])?;
        if !has_column {
            conn.execute_batch(MIGRATION_0023)?;
        }
        Ok(())
    }

    /// PRD-mcphost-agent-consent requirement 1: same new-table idempotency
    /// guard as 0021 above, gated on `contacts` (the first of the three
    /// tables -- plus the additive `messages.urgent` column -- this
    /// migration adds together, same "new table plus an additive column in
    /// one guarded batch" shape migration 0020 already uses). Renumbered
    /// 0023 -> 0024 on rebase onto mcphost-agent-wake, which landed its own
    /// migration 0023 first.
    fn migrate_0024_consent(conn: &Connection) -> Result<(), AppError> {
        let has_table: bool = conn
            .prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'contacts'")?
            .exists([])?;
        if !has_table {
            conn.execute_batch(MIGRATION_0024)?;
        }
        Ok(())
    }

    /// PRD-mcphost-tenant-self-offboard P1 requirement 5 / AC5: same
    /// additive-column idempotency guard as 0002/0020/0022 above, gated on
    /// `disabled_reason` (0025's only change). Renumbered 0023 -> 0025 on
    /// rebase onto mcphost-agent-wake + mcphost-agent-consent, which landed
    /// migrations 0023 and 0024 first (same renumbering precedent as
    /// `migrate_0024_consent` above).
    fn migrate_0025_self_offboard(conn: &Connection) -> Result<(), AppError> {
        let has_column: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('tenants') WHERE name = 'disabled_reason'")?
            .exists([])?;
        if !has_column {
            conn.execute_batch(MIGRATION_0025)?;
        }
        Ok(())
    }

    /// PRD-mcphost-data-retention P0 requirements 1-2: same new-table
    /// idempotency guard as 0011/0014/0015/0017/0019/0020/0021/0024 above,
    /// gated on `retention_policy`. `auto_vacuum=INCREMENTAL` must be set
    /// before the first table exists to take effect on a fresh database
    /// (technical considerations) -- but this migration runs 26th, long
    /// after migration 0001 created one, so the pragma alone would
    /// silently no-op on every already-deployed database. The one-time
    /// `VACUUM` immediately after (outside `MIGRATION_0026`'s own
    /// `execute_batch`, since `VACUUM` cannot run inside that call's
    /// implicit transaction) is what actually rebuilds the file under the
    /// new mode -- gated by this same `has_table` check so it only ever
    /// runs once per database file, matching the PRD's own "startup cost
    /// noted in the changelog" framing of a one-time cost, not a
    /// recurring one.
    fn migrate_0026_retention(conn: &Connection) -> Result<(), AppError> {
        let has_table: bool = conn
            .prepare(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'retention_policy'",
            )?
            .exists([])?;
        if !has_table {
            conn.execute_batch(MIGRATION_0026)?;
            conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
            conn.execute_batch("VACUUM;")?;
        }
        Ok(())
    }

    /// PRD-mcphost-agent-mesh-ops requirement 4/1: same additive-column
    /// idempotency guard as 0002/0020/0022/0025 above, gated on
    /// `mesh_frozen_at` -- the whole batch (the column plus the three new
    /// `channels`/`channel_posts`/`channel_cursors` tables) runs together,
    /// same "new tables plus an additive column in one guarded batch" shape
    /// migration 0024 already used.
    fn migrate_0027_mesh_ops(conn: &Connection) -> Result<(), AppError> {
        let has_column: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('tenants') WHERE name = 'mesh_frozen_at'")?
            .exists([])?;
        if !has_column {
            conn.execute_batch(MIGRATION_0027)?;
        }
        Ok(())
    }

    /// PRD-mcphost-tool-versions migration 0028 (requirement 1): same
    /// idempotency pattern as 0002-0027, gated on `tools.current_version`.
    /// Unlike 0013 (every existing tool defaults to `'private'` with no
    /// backfill pass needed), a real per-row `source_sha256` can't come from
    /// a column default -- [`backfill_tool_versions_sync`] gives every
    /// pre-existing tool its version-1 `tool_versions` row.
    fn migrate_0028_tool_versions(conn: &Connection) -> Result<(), AppError> {
        let has_column: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('tools') WHERE name = 'current_version'")?
            .exists([])?;
        if has_column {
            return Ok(());
        }
        conn.execute_batch(MIGRATION_0028)?;
        backfill_tool_versions_sync(conn)?;
        Ok(())
    }

    /// PRD-mcphost-agent-channels requirements 1/2/8, P1 requirement 10:
    /// same additive-column idempotency guard as 0002/0020/0022/0025/0027
    /// above, gated on `channels.group_id`.
    fn migrate_0029_agent_channels(conn: &Connection) -> Result<(), AppError> {
        let has_column: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('channels') WHERE name = 'group_id'")?
            .exists([])?;
        if !has_column {
            conn.execute_batch(MIGRATION_0029)?;
        }
        Ok(())
    }

    /// Same idempotency pattern as 0021/0026: `sqlite_master` gates the
    /// whole (pure `CREATE TABLE IF NOT EXISTS`) 0030 batch.
    fn migrate_0030_tool_lock(conn: &Connection) -> Result<(), AppError> {
        let has_table: bool = conn
            .prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'tool_lock'")?
            .exists([])?;
        if !has_table {
            conn.execute_batch(MIGRATION_0030)?;
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
                key_rotated_unix: None,
                disabled_reason: None,
                mesh_frozen_at: None,
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

    /// PRD-mcphost-agent-directory requirement 6: bumped from
    /// `handler::resolve_auth`/`resolve_tenant_key_auth` on every call that
    /// resolves to a live tenant. Best-effort from the caller's side (a
    /// failure here must never fail the request it rode in on).
    pub async fn touch_last_seen(&self, tenant_id: i64, unix: i64) -> Result<(), AppError> {
        self.with_conn(move |conn| {
            conn.execute(
                "UPDATE tenants SET last_seen_unix = ?1 WHERE id = ?2",
                params![unix, tenant_id],
            )?;
            Ok(())
        })
        .await
    }

    /// `host.agent.whoami`/`host.agent.profile_set` (requirements 2/3): the
    /// caller's own profile row, or the all-default row (P0 requirement 1)
    /// for a tenant that has never called `host.agent.profile_set`.
    pub async fn agent_profile(&self, tenant_id: i64) -> Result<AgentProfileRow, AppError> {
        self.with_conn(move |conn| Self::query_agent_profile(conn, tenant_id)).await
    }

    fn query_agent_profile(conn: &Connection, tenant_id: i64) -> Result<AgentProfileRow, AppError> {
        conn.query_row(
            "SELECT handle, description, tags_json, contact_policy FROM agent_profiles \
             WHERE tenant_id = ?1",
            params![tenant_id],
            agent_profile_row_from_row,
        )
        .optional()
        .map_err(AppError::from)
        .map(|row| {
            row.unwrap_or_else(|| AgentProfileRow {
                contact_policy: "open".to_string(),
                ..Default::default()
            })
        })
    }

    /// `host.agent.profile_set` (requirement 3 / AC3): `None` for a field
    /// means "leave unchanged"; for `handle`/`description`,
    /// `Some(None)` means "clear it" (an explicit JSON `null`) and
    /// `Some(Some(v))` means "set it" -- `v` is already validated and
    /// lower-cased by `agents::validate_handle` before this is called. The
    /// handle-uniqueness check and the upsert run inside one
    /// `BEGIN IMMEDIATE` transaction so a concurrent claim of the same
    /// handle can't both win (same hand-rolled-transaction pattern
    /// `Db::delete_tenant` already uses, needed here for the same reason:
    /// this runs inside `with_conn`'s `&Connection` closure, behind the
    /// single shared connection's mutex, so `Connection::transaction`'s
    /// `&mut Connection` isn't available).
    #[allow(clippy::too_many_arguments)]
    pub async fn set_agent_profile(
        &self,
        tenant_id: i64,
        handle: Option<Option<String>>,
        description: Option<Option<String>>,
        tags: Option<Vec<String>>,
        contact_policy: Option<String>,
    ) -> Result<SetProfileOutcome, AppError> {
        self.with_conn(move |conn| {
            conn.execute("BEGIN IMMEDIATE", []).map_err(AppError::from)?;
            let outcome: Result<SetProfileOutcome, AppError> = (|| {
                let current = Self::query_agent_profile(conn, tenant_id)?;
                let new_handle = match &handle {
                    None => current.handle.clone(),
                    Some(None) => None,
                    Some(Some(h)) => {
                        let taken: bool = conn
                            .prepare(
                                "SELECT 1 FROM agent_profiles WHERE handle = ?1 AND tenant_id != ?2",
                            )?
                            .exists(params![h, tenant_id])?;
                        if taken {
                            return Ok(SetProfileOutcome::HandleTaken);
                        }
                        Some(h.clone())
                    }
                };
                let new_description = match description {
                    None => current.description.clone(),
                    Some(d) => d,
                };
                let new_tags = tags.unwrap_or_else(|| current.tags.clone());
                let new_contact_policy = contact_policy.unwrap_or(current.contact_policy);
                let tags_json = serde_json::to_string(&new_tags).unwrap_or_else(|_| "[]".to_string());
                let now = now_rfc3339();
                conn.execute(
                    "INSERT INTO agent_profiles \
                         (tenant_id, handle, description, tags_json, contact_policy, updated_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
                     ON CONFLICT(tenant_id) DO UPDATE SET \
                         handle = excluded.handle, \
                         description = excluded.description, \
                         tags_json = excluded.tags_json, \
                         contact_policy = excluded.contact_policy, \
                         updated_at = excluded.updated_at",
                    params![tenant_id, new_handle, new_description, tags_json, new_contact_policy, now],
                )?;
                Ok(SetProfileOutcome::Ok(AgentProfileRow {
                    handle: new_handle,
                    description: new_description,
                    tags: new_tags,
                    contact_policy: new_contact_policy,
                }))
            })();
            match &outcome {
                Ok(SetProfileOutcome::Ok(_)) => {
                    conn.execute("COMMIT", []).map_err(AppError::from)?;
                }
                Ok(SetProfileOutcome::HandleTaken) | Err(_) => {
                    let _ = conn.execute("ROLLBACK", []);
                }
            }
            outcome
        })
        .await
    }

    /// `host.agent.lookup` (requirement 4 / AC2/AC4): `@handle` (matched
    /// case-insensitively against the lower-cased stored value) or a bare
    /// `t_...` namespace. Excludes disabled tenants -- a deleted tenant
    /// simply has no row -- so both collapse to the same `None`
    /// `agents::lookup` maps to the byte-identical `agent_not_found` (AC4).
    pub async fn lookup_agent(&self, address: String) -> Result<Option<AgentCard>, AppError> {
        self.lookup_agent_impl(address, false).await
    }

    /// `admin.agent.lookup` (P1 requirement 8): like [`Self::lookup_agent`]
    /// but on any tenant, disabled included -- the goal this tool exists
    /// for is recovering a squatted or abusive name, which requires seeing
    /// the tenant even if it's currently disabled.
    pub async fn lookup_agent_admin(&self, address: String) -> Result<Option<AgentCard>, AppError> {
        self.lookup_agent_impl(address, true).await
    }

    async fn lookup_agent_impl(
        &self,
        address: String,
        include_disabled: bool,
    ) -> Result<Option<AgentCard>, AppError> {
        self.with_conn(move |conn| {
            let disabled_clause = if include_disabled { "" } else { " AND t.disabled = 0" };
            if let Some(h) = address.strip_prefix('@') {
                let handle = h.to_lowercase();
                let sql = format!("{AGENT_CARD_SELECT} WHERE ap.handle = ?1{disabled_clause}");
                conn.query_row(&sql, params![handle], agent_card_from_row)
                    .optional()
                    .map_err(AppError::from)
            } else {
                let sql = format!("{AGENT_CARD_SELECT} WHERE t.namespace = ?1{disabled_clause}");
                conn.query_row(&sql, params![address], agent_card_from_row)
                    .optional()
                    .map_err(AppError::from)
            }
        })
        .await
    }

    /// `host.agent.search` (requirement 5 / AC5/AC6): every enabled
    /// tenant's card, in the exact order the PRD promises (`handle NULLS
    /// LAST, namespace`, expressed here as `ap.handle IS NULL, ap.handle`
    /// since SQLite sorts NULL first by default). `agents::search` does the
    /// `tag`/`query` filtering and offset pagination in Rust rather than
    /// pushing them into SQL -- neither field is indexed, and the PRD's own
    /// latency target (AC8) is scoped to `host.agent.lookup`, not search.
    pub async fn list_agent_cards(&self) -> Result<Vec<AgentCard>, AppError> {
        self.with_conn(|conn| {
            let sql = format!(
                "{AGENT_CARD_SELECT} WHERE t.disabled = 0 \
                 ORDER BY ap.handle IS NULL, ap.handle, t.namespace"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map([], agent_card_from_row)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
        .await
    }

    /// `admin.agent.handle_release` (P1 requirement 8 / AC9): frees a handle
    /// regardless of which tenant holds it and writes one `admin_events` row
    /// (migration 0005's table -- same audit trail `Db::delete_tenant`
    /// writes to; distinct from `admin_audit`, which
    /// `handler::dispatch_admin_tool` already appends to centrally for every
    /// admin.* mutation). Idempotent: releasing an already-free handle is
    /// not an error, just a no-op `false`.
    pub async fn release_handle(&self, handle: String) -> Result<bool, AppError> {
        self.with_conn(move |conn| {
            let changed = conn.execute(
                "UPDATE agent_profiles SET handle = NULL, updated_at = ?2 WHERE handle = ?1",
                params![handle, now_rfc3339()],
            )? > 0;
            if changed {
                conn.execute(
                    "INSERT INTO admin_events (ts, action, tenant, detail) VALUES (?1, ?2, NULL, ?3)",
                    params![now_rfc3339(), "agent_handle_release", handle],
                )?;
            }
            Ok(changed)
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

    /// `admin.tenant_disable`/`admin.tenant_enable`'s own flip.
    /// PRD-mcphost-tenant-self-offboard P1 requirement 5 / AC5: enabling
    /// (`disabled = false`) also clears `disabled_reason` back to `NULL`
    /// -- a re-enabled tenant carries no stale "why it was disabled" label
    /// -- and disabling via this path always records `"admin_disable"`,
    /// distinguishing it from [`Self::self_offboard_tenant`]'s
    /// `"self_offboard"`.
    pub async fn set_tenant_disabled(
        &self,
        namespace: String,
        disabled: bool,
    ) -> Result<bool, AppError> {
        self.with_conn(move |conn| {
            let reason: Option<&str> = if disabled { Some("admin_disable") } else { None };
            let n = conn.execute(
                "UPDATE tenants SET disabled = ?1, disabled_reason = ?2 WHERE namespace = ?3",
                params![disabled as i64, reason, namespace],
            )?;
            Ok(n > 0)
        })
        .await
    }

    /// `host.self_offboard()` (PRD-mcphost-tenant-self-offboard P0
    /// requirement 1 / AC1-2): the tenant's own public path to disable its
    /// account, keyed by `tenant_id` (not `namespace` -- the caller already
    /// holds a resolved [`Tenant`], same convention as
    /// [`Self::rotate_tenant_key`]). Records `disabled_reason =
    /// 'self_offboard'` (AC5) so it's distinguishable from an
    /// `admin.tenant_disable` row. `WHERE disabled = 0` makes this a no-op
    /// (not an error) on a tenant that is somehow already disabled by the
    /// time this runs -- defense in depth alongside the auth-layer
    /// idempotency `dispatch_tenant_tool` already provides (a disabled
    /// tenant's key never resolves to a call reaching this method at all;
    /// see `control::self_offboard`'s doc comment for AC2's actual
    /// mechanism), so a direct future caller of this method that skips
    /// that gate still can't flip `disabled_reason` back to
    /// `'self_offboard'` on a row an admin has since re-enabled and
    /// disabled again for a different reason.
    pub async fn self_offboard_tenant(&self, tenant_id: i64) -> Result<bool, AppError> {
        self.with_conn(move |conn| {
            let n = conn.execute(
                "UPDATE tenants SET disabled = 1, disabled_reason = 'self_offboard' \
                 WHERE id = ?1 AND disabled = 0",
                params![tenant_id],
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

    /// PRD-mcphost-handoff-token requirement 3 / AC3: replace this tenant's
    /// key hash in place and stamp `key_rotated_unix` -- one statement, so
    /// the old key stops resolving (`find_tenant_by_key_hash` on its hash
    /// returns nothing) the instant this returns, with no window where both
    /// the old and new hash would authenticate.
    pub async fn rotate_tenant_key(&self, tenant_id: i64, new_key_hash: String) -> Result<(), AppError> {
        let now = now_unix();
        self.with_conn(move |conn| {
            conn.execute(
                "UPDATE tenants SET key_hash = ?1, key_rotated_unix = ?2 WHERE id = ?3",
                params![new_key_hash, now, tenant_id],
            )?;
            Ok(())
        })
        .await
    }

    /// PRD-mcphost-handoff-token requirement 1: record a freshly issued
    /// handoff token. `key_enc`/`key_nonce` are the raw tenant key,
    /// AES-256-GCM-encrypted by the caller (`control::signup`, via
    /// `AppState::secrets`) -- this method never sees (or could log) the
    /// plaintext key. Returns the new row's id, used only for journaling
    /// (never the token or key value -- requirement 2's "journaled: token
    /// id, never values").
    #[allow(clippy::too_many_arguments)]
    pub async fn create_handoff_token(
        &self,
        tenant_id: i64,
        token_hash: String,
        key_enc: Vec<u8>,
        key_nonce: Vec<u8>,
        expires_unix: i64,
    ) -> Result<i64, AppError> {
        let created_unix = now_unix();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO handoff_tokens (tenant_id, token_hash, key_enc, key_nonce, \
                 expires_unix, created_unix) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![tenant_id, token_hash, key_enc, key_nonce, expires_unix, created_unix],
            )?;
            Ok(conn.last_insert_rowid())
        })
        .await
    }

    /// PRD-mcphost-handoff-token requirement 2 / AC2: consult, then
    /// atomically claim, one handoff token by its hash. The claim is a
    /// second statement (`UPDATE ... WHERE redeemed_unix IS NULL`) rather
    /// than folding the whole decision into one query, so two concurrent
    /// redemptions of the same token can't both read "not yet redeemed" and
    /// both win -- only the `UPDATE` that actually flips the row from NULL
    /// wins; the loser sees `claimed == 0` and reports `AlreadyRedeemed`,
    /// same as a genuine second, later redemption would.
    pub async fn redeem_handoff_token(
        &self,
        token_hash: String,
    ) -> Result<HandoffRedeemOutcome, AppError> {
        let now = now_unix();
        self.with_conn(move |conn| {
            struct Row {
                id: i64,
                tenant_id: i64,
                expires_unix: i64,
                redeemed_unix: Option<i64>,
                key_enc: Vec<u8>,
                key_nonce: Vec<u8>,
            }
            let row: Option<Row> = conn
                .query_row(
                    "SELECT id, tenant_id, expires_unix, redeemed_unix, key_enc, key_nonce \
                     FROM handoff_tokens WHERE token_hash = ?1",
                    params![token_hash],
                    |r| {
                        Ok(Row {
                            id: r.get(0)?,
                            tenant_id: r.get(1)?,
                            expires_unix: r.get(2)?,
                            redeemed_unix: r.get(3)?,
                            key_enc: r.get(4)?,
                            key_nonce: r.get(5)?,
                        })
                    },
                )
                .optional()?;
            let Some(Row {
                id,
                tenant_id,
                expires_unix,
                redeemed_unix,
                key_enc,
                key_nonce,
            }) = row
            else {
                return Ok(HandoffRedeemOutcome::NotFound);
            };
            if redeemed_unix.is_some() {
                return Ok(HandoffRedeemOutcome::AlreadyRedeemed { token_id: id });
            }
            if now > expires_unix {
                return Ok(HandoffRedeemOutcome::Expired { token_id: id });
            }
            let claimed = conn.execute(
                "UPDATE handoff_tokens SET redeemed_unix = ?1 \
                 WHERE token_hash = ?2 AND redeemed_unix IS NULL",
                params![now, token_hash],
            )?;
            if claimed == 0 {
                return Ok(HandoffRedeemOutcome::AlreadyRedeemed { token_id: id });
            }
            Ok(HandoffRedeemOutcome::Redeemed {
                token_id: id,
                tenant_id,
                key_enc,
                key_nonce,
            })
        })
        .await
    }

    /// Test-only: force one handoff token past its expiry without a real
    /// sleep -- same "flip an internal knob for a test" shape as
    /// [`Self::set_query_only`] (AC14) above.
    pub async fn expire_handoff_token_for_test(&self, token_hash: String) -> Result<(), AppError> {
        self.with_conn(move |conn| {
            conn.execute(
                "UPDATE handoff_tokens SET expires_unix = 0 WHERE token_hash = ?1",
                params![token_hash],
            )?;
            Ok(())
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
    ///
    /// PRD-mcphost-tool-versions requirement 2 (AC1/AC3): every publish is
    /// also a new, immutable `tool_versions` row -- `version` is
    /// `MAX(version)+1` over that tool's own surviving rows (never reused,
    /// even across a rollback that moves `tools.current_version` backward,
    /// since retention only ever prunes the OLDEST rows, never the highest
    /// version number issued) -- and `tools.current_version` advances to
    /// match. Returns the new version number. `versions_max` (the tenant's
    /// plan `versions_max`) is enforced here, deleting the oldest surviving
    /// versions beyond it -- never the row just inserted.
    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_tool(
        &self,
        tenant_id: i64,
        name: String,
        kind: String,
        spec: Value,
        versions_max: i64,
    ) -> Result<i64, AppError> {
        let created_at = now_rfc3339();
        let created_unix = now_unix();
        let spec_text = serde_json::to_string(&spec)
            .map_err(|e| AppError::Internal(format!("spec serialize: {e}")))?;
        let source_sha256 = sha256_hex(spec_text.as_bytes());
        self.with_conn(move |conn| {
            let next_version: i64 = conn.query_row(
                "SELECT COALESCE(MAX(version), 0) + 1 FROM tool_versions WHERE tenant_id = ?1 AND name = ?2",
                params![tenant_id, name],
                |r| r.get(0),
            )?;
            conn.execute(
                "INSERT INTO tool_versions \
                 (tenant_id, name, version, kind, spec, created_at, created_unix, source_sha256) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    tenant_id,
                    name,
                    next_version,
                    kind,
                    spec_text,
                    created_at,
                    created_unix,
                    source_sha256
                ],
            )?;
            conn.execute(
                "INSERT INTO tools (tenant_id, name, kind, spec, created_at, current_version) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
                 ON CONFLICT(tenant_id, name) DO UPDATE SET \
                 kind = excluded.kind, spec = excluded.spec, current_version = excluded.current_version",
                params![tenant_id, name, kind, spec_text, created_at, next_version],
            )?;
            // requirement 2 / AC3: oldest versions beyond `versions_max` are
            // deleted oldest-first -- the KEEP set is the newest
            // `versions_max` rows by version number, which always includes
            // the row just inserted above.
            conn.execute(
                "DELETE FROM tool_versions WHERE tenant_id = ?1 AND name = ?2 AND version NOT IN ( \
                     SELECT version FROM tool_versions WHERE tenant_id = ?1 AND name = ?2 \
                     ORDER BY version DESC LIMIT ?3 \
                 )",
                params![tenant_id, name, versions_max],
            )?;
            touch_tenant_tool_change(conn, tenant_id)?;
            Ok(next_version)
        })
        .await
    }

    /// PRD-mcphost-tool-versions requirement 3 (AC1): every version of
    /// `name`, newest-first.
    pub async fn list_tool_versions(
        &self,
        tenant_id: i64,
        name: String,
    ) -> Result<Vec<ToolVersionRow>, AppError> {
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT version, kind, spec, created_at, created_unix, source_sha256 \
                 FROM tool_versions WHERE tenant_id = ?1 AND name = ?2 ORDER BY version DESC",
            )?;
            let rows = stmt
                .query_map(params![tenant_id, name], tool_version_from_row)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// A single version's own kind/spec (requirement 5 / AC5: a
    /// version-pinned call dispatches against this instead of `tools.spec`;
    /// requirement 8 / AC9: `host.tool_diff` reads both ends of the diff
    /// from here).
    pub async fn get_tool_version(
        &self,
        tenant_id: i64,
        name: String,
        version: i64,
    ) -> Result<Option<ToolVersionRow>, AppError> {
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT version, kind, spec, created_at, created_unix, source_sha256 \
                 FROM tool_versions WHERE tenant_id = ?1 AND name = ?2 AND version = ?3",
                params![tenant_id, name, version],
                tool_version_from_row,
            )
            .optional()
            .map_err(AppError::from)
        })
        .await
    }

    /// The lowest/highest surviving version number for `name`, `None` when
    /// the tool has no versions at all (a since-removed tool) -- what
    /// [`AppError::version_not_found`]'s range comes from (AC4).
    pub async fn tool_version_range(
        &self,
        tenant_id: i64,
        name: String,
    ) -> Result<Option<(i64, i64)>, AppError> {
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT MIN(version), MAX(version) FROM tool_versions WHERE tenant_id = ?1 AND name = ?2",
                params![tenant_id, name],
                |r| {
                    let min: Option<i64> = r.get(0)?;
                    let max: Option<i64> = r.get(1)?;
                    Ok(min.zip(max))
                },
            )
            .map_err(AppError::from)
        })
        .await
    }

    // ---- dependency lock (PRD-mcphost-python-dependency-policy) --------

    /// requirement 1/2: stores this version's resolved dependency lock,
    /// including whatever advisories `deps::resolve` already found at
    /// publish time (requirement 2: `MCPHOST_ADVISORY_MODE=warn` lets a
    /// known advisory through with the publish still succeeding -- this is
    /// how that gets recorded rather than silently dropped). `audited_unix`
    /// starts `NULL`: a later re-audit (requirement 6) is what sets it.
    #[allow(clippy::too_many_arguments)]
    pub async fn store_tool_lock(
        &self,
        tenant_id: i64,
        name: String,
        version: i64,
        lock_text: String,
        resolved_unix: i64,
        advisories_json: String,
    ) -> Result<(), AppError> {
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO tool_lock \
                 (tenant_id, name, version, lock_text, resolved_unix, advisories_json, audited_unix) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL) \
                 ON CONFLICT(tenant_id, name, version) DO UPDATE SET \
                 lock_text = excluded.lock_text, resolved_unix = excluded.resolved_unix, \
                 advisories_json = excluded.advisories_json, audited_unix = NULL",
                params![tenant_id, name, version, lock_text, resolved_unix, advisories_json],
            )?;
            Ok(())
        })
        .await
    }

    /// requirement 6 (AC8): every currently-active tool's dependency lock,
    /// across every tenant -- what the daily re-audit (or its on-demand
    /// `admin.dependency_reaudit` trigger) iterates. `(tenant_id, name,
    /// version, lock_text)`.
    pub async fn list_current_tool_locks(&self) -> Result<Vec<(i64, String, i64, String)>, AppError> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT tl.tenant_id, tl.name, tl.version, tl.lock_text \
                 FROM tool_lock tl JOIN tools t \
                 ON t.tenant_id = tl.tenant_id AND t.name = tl.name \
                 WHERE tl.version = t.current_version",
            )?;
            let rows = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// requirement 6 (AC8): records a fresh re-audit result for one
    /// version's lock -- the only writer of `advisories_json`/`audited_unix`
    /// past publish time.
    #[allow(clippy::too_many_arguments)]
    pub async fn update_tool_lock_advisories(
        &self,
        tenant_id: i64,
        name: String,
        version: i64,
        advisories_json: String,
        audited_unix: i64,
    ) -> Result<(), AppError> {
        self.with_conn(move |conn| {
            conn.execute(
                "UPDATE tool_lock SET advisories_json = ?1, audited_unix = ?2 \
                 WHERE tenant_id = ?3 AND name = ?4 AND version = ?5",
                params![advisories_json, audited_unix, tenant_id, name, version],
            )?;
            Ok(())
        })
        .await
    }

    /// requirement 7 (AC9): current-version advisory counts for every
    /// python tool with a stored lock, across every tenant, keyed by
    /// `(tenant_id, name)` -- read fresh from `tool_lock` (not the tool's
    /// immutable stored `spec`) so a re-audit's update is visible here
    /// without a republish, same reasoning as `host.tool_list`'s own
    /// `advisories` field (see `control::tool_list`).
    pub async fn current_tool_lock_advisory_counts(
        &self,
    ) -> Result<HashMap<(i64, String), i64>, AppError> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT tl.tenant_id, tl.name, tl.advisories_json \
                 FROM tool_lock tl JOIN tools t \
                 ON t.tenant_id = tl.tenant_id AND t.name = tl.name \
                 WHERE tl.version = t.current_version",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    let tenant_id: i64 = r.get(0)?;
                    let name: String = r.get(1)?;
                    let advisories_json: String = r.get(2)?;
                    Ok((tenant_id, name, advisories_json))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows
                .into_iter()
                .map(|(tenant_id, name, advisories_json)| {
                    let count = serde_json::from_str::<Vec<Value>>(&advisories_json)
                        .map(|v| v.len() as i64)
                        .unwrap_or(0);
                    ((tenant_id, name), count)
                })
                .collect())
        })
        .await
    }

    /// requirement 6 (AC8): the same per-tool advisory count as
    /// [`Self::current_tool_lock_advisory_counts`], scoped to one tenant
    /// and keyed by bare tool name -- what `control::tool_list`'s
    /// `advisories` field reads (a tenant only ever lists its own tools).
    pub async fn current_tool_lock_advisory_counts_for_tenant(
        &self,
        tenant_id: i64,
    ) -> Result<HashMap<String, i64>, AppError> {
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT tl.name, tl.advisories_json \
                 FROM tool_lock tl JOIN tools t \
                 ON t.tenant_id = tl.tenant_id AND t.name = tl.name \
                 WHERE tl.tenant_id = ?1 AND tl.version = t.current_version",
            )?;
            let rows = stmt
                .query_map(params![tenant_id], |r| {
                    let name: String = r.get(0)?;
                    let advisories_json: String = r.get(1)?;
                    Ok((name, advisories_json))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows
                .into_iter()
                .map(|(name, advisories_json)| {
                    let count = serde_json::from_str::<Vec<Value>>(&advisories_json)
                        .map(|v| v.len() as i64)
                        .unwrap_or(0);
                    (name, count)
                })
                .collect())
        })
        .await
    }

    /// requirement 7 (AC9): every tenant's every tool's `tenant_id`/`name`/
    /// `kind`/`spec`, host-wide -- what `admin.usage`'s network-mode tally
    /// reads (python specs only; other kinds have no network-mode concept
    /// and are skipped by the caller). `tenant_id`/`name` are what joins
    /// this against [`Self::current_tool_lock_advisory_counts`]'s own key.
    pub async fn list_all_tool_specs(&self) -> Result<Vec<(i64, String, String, Value)>, AppError> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare("SELECT tenant_id, name, kind, spec FROM tools")?;
            let rows = stmt
                .query_map([], |r| {
                    let tenant_id: i64 = r.get(0)?;
                    let name: String = r.get(1)?;
                    let kind: String = r.get(2)?;
                    let spec_text: String = r.get(3)?;
                    Ok((tenant_id, name, kind, spec_text))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows
                .into_iter()
                .filter_map(|(tenant_id, name, kind, spec_text)| {
                    serde_json::from_str::<Value>(&spec_text)
                        .ok()
                        .map(|spec| (tenant_id, name, kind, spec))
                })
                .collect())
        })
        .await
    }

    /// PRD-mcphost-tool-versions requirement 4 (AC2/AC4): sets
    /// `tools.current_version`/`kind`/`spec` to `version`'s own stored
    /// content and triggers whatever it takes for the next unpinned call to
    /// see it (the caller, `control::tool_rollback`, notifies every `Kind`,
    /// same convention as `remove_tool`). Refuses an unknown version by
    /// naming the surviving range rather than a bare not-found.
    pub async fn rollback_tool_version(
        &self,
        tenant_id: i64,
        name: String,
        version: i64,
    ) -> Result<RollbackOutcome, AppError> {
        self.with_conn(move |conn| {
            let exists: bool = conn
                .prepare("SELECT 1 FROM tools WHERE tenant_id = ?1 AND name = ?2")?
                .exists(params![tenant_id, name])?;
            if !exists {
                return Ok(RollbackOutcome::NotFound);
            }
            let target: Option<(String, String)> = conn
                .query_row(
                    "SELECT kind, spec FROM tool_versions WHERE tenant_id = ?1 AND name = ?2 AND version = ?3",
                    params![tenant_id, name, version],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((kind, spec)) = target else {
                let (min, max): (Option<i64>, Option<i64>) = conn.query_row(
                    "SELECT MIN(version), MAX(version) FROM tool_versions WHERE tenant_id = ?1 AND name = ?2",
                    params![tenant_id, name],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
                return Ok(RollbackOutcome::OutOfRange {
                    min: min.unwrap_or(1),
                    max: max.unwrap_or(1),
                });
            };
            conn.execute(
                "UPDATE tools SET kind = ?1, spec = ?2, current_version = ?3 WHERE tenant_id = ?4 AND name = ?5",
                params![kind, spec, version, tenant_id, name],
            )?;
            touch_tenant_tool_change(conn, tenant_id)?;
            Ok(RollbackOutcome::Ok { kind })
        })
        .await
    }

    /// PRD-mcphost-tool-versions requirement 6 (AC7): compares `name`'s
    /// current version to the last one `caller_tenant_id` was ever recorded
    /// seeing on an unpinned call, always upserting the watermark to
    /// `current_version` -- returns `Some(previous)` exactly when this is a
    /// real change (so `version_changed` is reported once), `None` on a
    /// first-ever call (no prior watermark: nothing to compare, not a
    /// "change") or an unchanged one.
    pub async fn check_version_watermark(
        &self,
        owner_tenant_id: i64,
        name: String,
        caller_tenant_id: i64,
        current_version: i64,
    ) -> Result<Option<i64>, AppError> {
        self.with_conn(move |conn| {
            let previous: Option<i64> = conn
                .query_row(
                    "SELECT last_version FROM tool_version_watermarks \
                     WHERE tenant_id = ?1 AND name = ?2 AND caller_tenant_id = ?3",
                    params![owner_tenant_id, name, caller_tenant_id],
                    |r| r.get(0),
                )
                .optional()?;
            conn.execute(
                "INSERT INTO tool_version_watermarks (tenant_id, name, caller_tenant_id, last_version) \
                 VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT(tenant_id, name, caller_tenant_id) DO UPDATE SET last_version = excluded.last_version",
                params![owner_tenant_id, name, caller_tenant_id, current_version],
            )?;
            Ok(match previous {
                Some(prev) if prev != current_version => Some(prev),
                _ => None,
            })
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
                            current_version: r.get(12)?,
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
                        current_version: r.get(12)?,
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
                // PRD-mcphost-tool-versions requirement 7 (AC8): a remove
                // takes every version (and any sharer watermark) with it --
                // `host.tool_history` on a removed name must read back as
                // `tool_not_found`, not an empty version list, which
                // `control::tool_history`'s own `get_tool` check gives it
                // once the `tools` row above is gone; these two deletes just
                // keep no orphaned history behind for a name later reused.
                conn.execute(
                    "DELETE FROM tool_versions WHERE tenant_id = ?1 AND name = ?2",
                    params![tenant_id, name],
                )?;
                conn.execute(
                    "DELETE FROM tool_version_watermarks WHERE tenant_id = ?1 AND name = ?2",
                    params![tenant_id, name],
                )?;
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
        // PRD-mcphost-inbound-events P0 requirement 3 / migration 0018 /
        // AC7: `true` only for `host.trigger.test`'s own enqueue; every
        // other caller (an ordinary async call, the scheduler tick,
        // `POST /hooks/...`, `host.trigger.replay`) passes `false`.
        test: bool,
        // PRD-mcphost-agent-wake requirement 2 / migration 0023: `Some` only
        // for a `trigger = "message"` run's real, stored message id; every
        // other caller (including a message trigger's own
        // `host.trigger.test`, whose envelope is synthetic) passes `None`.
        message_id: Option<String>,
    ) -> Result<(), AppError> {
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO runs (id, tenant_id, tool_name, trigger, trigger_ref, \
                 caller_tenant_id, status, deadline_s, attempt, args_json, manual, test_run, message_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'queued', ?7, 1, ?8, ?9, ?10, ?11)",
                params![run_id, tenant_id, tool_name, trigger, trigger_ref, caller_tenant_id, deadline_s, args_json, manual, test, message_id],
            )?;
            Ok(())
        })
        .await
    }

    /// PRD-mcphost-tenant-data-export P0 requirement 3 / AC3: atomically
    /// checks for an already-`running` export for this tenant and returns
    /// its id instead of starting a second one. The check-then-insert
    /// happens inside one [`Self::with_conn`] closure -- this crate's
    /// single-writer SQLite connection is serialized by that method's own
    /// mutex (the same "no separate lock needed" property
    /// [`Self::lease_next_queued_run`]'s own doc comment relies on) -- so
    /// two concurrent `host.export` calls can never both insert a `running`
    /// row.
    ///
    /// Unlike [`Self::insert_queued_run`], this inserts a row already
    /// `running` (never `queued`): the executor's own `tick()` only ever
    /// leases `queued` rows and dispatches them through a tenant's
    /// published `Kind` (see `runs::execute_job`'s `db.get_tool` lookup),
    /// and an export has no `tools` row or `Kind` to dispatch through --
    /// inserting straight to `running` keeps it invisible to that leasing
    /// loop entirely, so only `export::run_export_job` ever finalizes it.
    #[allow(clippy::too_many_arguments)]
    pub async fn start_export_run(
        &self,
        tenant_id: i64,
        run_id: String,
        tool_name: String,
        deadline_s: i64,
        args_json: String,
    ) -> Result<StartExportRun, AppError> {
        let now = now_unix();
        self.with_conn(move |conn| {
            let existing: Option<String> = conn
                .query_row(
                    "SELECT id FROM runs WHERE tenant_id = ?1 AND tool_name = ?2 \
                     AND status = 'running' ORDER BY rowid LIMIT 1",
                    params![tenant_id, tool_name],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(id) = existing {
                return Ok(StartExportRun::AlreadyRunning(id));
            }
            conn.execute(
                "INSERT INTO runs (id, tenant_id, tool_name, trigger, status, started_unix, \
                 deadline_s, attempt, args_json) \
                 VALUES (?1, ?2, ?3, 'job', 'running', ?4, ?5, 1, ?6)",
                params![run_id, tenant_id, tool_name, now, deadline_s, args_json],
            )?;
            Ok(StartExportRun::Started(run_id.clone()))
        })
        .await
    }

    /// PRD-mcphost-tenant-data-export P0 requirement 2: `GET /exports/{run_id}`
    /// has no bearer/tenant context (a signed URL is the whole auth story),
    /// so this looks a run up by id alone -- unlike [`Self::get_run`], which
    /// is deliberately scoped to a caller's own tenant.
    pub async fn find_run_by_id(&self, run_id: String) -> Result<Option<RunRow>, AppError> {
        self.with_conn(move |conn| {
            conn.query_row(
                &format!("SELECT {RUN_COLUMNS} FROM runs WHERE id = ?1"),
                params![run_id],
                run_row_from_row,
            )
            .optional()
            .map_err(AppError::from)
        })
        .await
    }

    /// PRD-mcphost-agent-wake requirement 4 / AC4: an enqueue attempt that
    /// lost the `jobs_concurrent` admission check
    /// (`hooks::enqueue_with_dedupe`'s own quota check, mirroring the
    /// executor's own `jobs_concurrent` admission -- [`Self::count_running_runs_for_tenant`])
    /// still writes a run row, already terminal (`rejected`, never leased),
    /// so the firing is visible in `host.runs.list` rather than silently
    /// dropped -- same "insert already-terminal" shape
    /// [`Self::insert_skipped_run`] uses for a schedule's own overlap skip.
    ///
    /// Takes [`RejectedRun`] rather than its seven fields positionally --
    /// clippy::too_many_arguments (crate threshold 5, `clippy.toml`) aside,
    /// `trigger`/`trigger_ref`/`message_id` are three adjacent `String`-typed
    /// fields a positional call site could silently transpose (the exact
    /// risk `extend-gate.sh`'s reviewer-agent flagged on this function);
    /// named fields at the call site make that a compile-time-obvious typo
    /// instead.
    pub async fn insert_rejected_run(&self, run: RejectedRun) -> Result<(), AppError> {
        let RejectedRun {
            run_id,
            tenant_id,
            tool_name,
            trigger,
            trigger_ref,
            message_id,
            error_class,
        } = run;
        let now = now_unix();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO runs (id, tenant_id, tool_name, trigger, trigger_ref, message_id, \
                 status, error_class, started_unix, finished_unix, duration_ms, attempt) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'rejected', ?7, ?8, ?8, 0, 1)",
                params![run_id, tenant_id, tool_name, trigger, trigger_ref, message_id, error_class, now],
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
    /// PRD-mcphost-tenant-data-export P2 requirement 6 / AC7: `admin.usage`'s
    /// `exports_today` -- every `runs` row for `tool_name` started at or
    /// after `since_unix` (the caller's own "today" boundary), across every
    /// tenant. `runs.started_unix` is stamped at [`Self::start_export_run`]'s
    /// insert time (never `queued`, so this is also the row's creation
    /// time), the same column [`Self::count_running_runs_total`] and
    /// friends already read.
    pub async fn count_runs_by_tool_since(
        &self,
        tool_name: String,
        since_unix: i64,
    ) -> Result<i64, AppError> {
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM runs WHERE tool_name = ?1 AND started_unix >= ?2",
                params![tool_name, since_unix],
                |r| r.get(0),
            )
            .map_err(AppError::from)
        })
        .await
    }

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

    /// PRD-mcphost-inbound-events P0 requirement 4's quota check: how many
    /// `kind = 'event'` (and, since PRD-mcphost-agent-wake P0 requirement
    /// 1, `kind = 'message'`) triggers this tenant holds right now, paused
    /// or not -- same "only remove frees a slot" rule
    /// [`Self::count_schedule_triggers_for_tenant`] already applies to
    /// schedules. A message trigger sharing this quota (rather than a
    /// separate `message_triggers_max`) is requirement 1's own choice --
    /// both are "a tool fires on some inbound signal" triggers, and giving
    /// them one shared ceiling means an existing plan config needs no new
    /// field to already bound the new kind.
    pub async fn count_event_triggers_for_tenant(&self, tenant_id: i64) -> Result<i64, AppError> {
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM triggers WHERE tenant_id = ?1 AND kind IN ('event', 'message')",
                params![tenant_id],
                |r| r.get(0),
            )
            .map_err(AppError::from)
        })
        .await
    }

    /// PRD-mcphost-agent-wake requirement 2: every enabled `message`-kind
    /// trigger a recipient holds, across every tool -- the delivery-time
    /// lookup `messaging::fire_message_triggers` does on every
    /// `host.msg.send`/`reply`, scoped by migration 0023's
    /// `idx_triggers_tenant_kind_enabled` index (technical considerations:
    /// "under 20ms").
    pub async fn list_enabled_message_triggers(&self, tenant_id: i64) -> Result<Vec<TriggerRow>, AppError> {
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {TRIGGER_COLUMNS} FROM triggers \
                 WHERE tenant_id = ?1 AND kind = 'message' AND enabled = 1"
            ))?;
            let rows = stmt
                .query_map(params![tenant_id], trigger_row_from_row)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
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

    // ---- event dedupe (PRD-mcphost-inbound-events P1 requirement 7) ------

    /// Attempts to claim `(trigger_id, dedupe_key)` for `run_id`: `None` if
    /// this is the first delivery of that key within [`EVENT_DEDUPE_WINDOW_S`]
    /// (the claim succeeded, the caller should enqueue `run_id` for real),
    /// `Some(existing_run_id)` if a still-fresh delivery already claimed it
    /// (AC11: "the second answers 202 with the first run id and no second
    /// run exists"). A claim older than the window is overwritten (`INSERT
    /// OR REPLACE`) rather than blocked forever, matching requirement 7's
    /// own "within 24 h" wording -- a sender that genuinely reuses a
    /// delivery id a week later gets a fresh run, not a permanently stale
    /// one. One connection, one `with_conn` call: SQLite's single-writer
    /// mutex already serializes the read-then-write below, so no separate
    /// transaction is needed to avoid a race between two callers.
    pub async fn claim_event_dedupe(
        &self,
        trigger_id: String,
        dedupe_key: String,
        run_id: String,
    ) -> Result<Option<String>, AppError> {
        let now = now_unix();
        self.with_conn(move |conn| {
            let existing: Option<(String, i64)> = conn
                .query_row(
                    "SELECT run_id, created_unix FROM event_dedupe \
                     WHERE trigger_id = ?1 AND dedupe_key = ?2",
                    params![trigger_id, dedupe_key],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if let Some((existing_run_id, created_unix)) = existing
                && now - created_unix < EVENT_DEDUPE_WINDOW_S
            {
                return Ok(Some(existing_run_id));
            }
            conn.execute(
                "INSERT OR REPLACE INTO event_dedupe (trigger_id, dedupe_key, run_id, created_unix) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![trigger_id, dedupe_key, run_id, now],
            )?;
            Ok(None)
        })
        .await
    }

    // ---- messaging (PRD-mcphost-agent-inbox) ----------------------------

    /// Resolve `address` (an `@handle` or a bare `t_...` namespace) to a
    /// live, addressable tenant for `host.msg.send`/`reply`: unknown/
    /// disabled resolves to `agent_not_found` right here; every other
    /// refusal (blocked, `closed`, `contacts` with no accepted/active
    /// request) is [`crate::consent::check`]'s decision -- the ONE shared
    /// function requirement 4's technical considerations ask for, so this
    /// and `Db::msg_reply`'s per-participant loop can't drift the way they
    /// used to (each carried its own copy of the blocked/policy check
    /// before this PRD).
    fn resolve_message_recipient(
        conn: &Connection,
        address: &str,
        sender_id: i64,
        now_ms: i64,
    ) -> Result<RecipientResolution, AppError> {
        let tenant = if let Some(h) = address.strip_prefix('@') {
            let handle = h.to_lowercase();
            let sql = format!(
                "SELECT {TENANT_COLUMNS} FROM tenants t JOIN agent_profiles ap ON ap.tenant_id = t.id \
                 WHERE ap.handle = ?1 AND t.disabled = 0"
            );
            conn.query_row(&sql, params![handle], tenant_from_row)
                .optional()?
        } else {
            let sql = format!("SELECT {TENANT_COLUMNS} FROM tenants t WHERE t.namespace = ?1 AND t.disabled = 0");
            conn.query_row(&sql, params![address], tenant_from_row)
                .optional()?
        };
        let Some(tenant) = tenant else {
            return Ok(RecipientResolution::Refused("agent_not_found"));
        };
        let policy = Self::query_agent_profile(conn, tenant.id)?.contact_policy;
        match crate::consent::check(conn, sender_id, tenant.id, &policy, now_ms)? {
            crate::consent::SendCheck::Allow => Ok(RecipientResolution::Ok(Box::new(tenant))),
            crate::consent::SendCheck::Refuse(code) => Ok(RecipientResolution::Refused(code)),
        }
    }

    /// `host.msg.send` (requirements 2, 6, 8, 11 / AC1, AC4, AC5, AC7, AC9,
    /// AC10, AC14): creates a new thread (`thread_id: None`) or adds to one
    /// the caller already participates in (requirement 11); resolves every
    /// `to` address, skipping refused ones rather than failing the whole
    /// call; allocates the next `seq`; and honors a `dedupe_key` resend
    /// within [`MSG_DEDUPE_WINDOW_MS`] by returning the original
    /// `message_id` and storing nothing new. After the window, the stale
    /// row's `dedupe_key` is nulled (never the row itself -- the original
    /// message stays in history) so `UNIQUE(from_tenant_id, dedupe_key)`
    /// doesn't block the key's reuse (requirement 8: "after 24h the key is
    /// free").
    ///
    /// Quota checks that fail the *whole call* (`msgs_per_hour`,
    /// `msg_body_bytes_max`, `recipients_per_msg_max`) are the caller's
    /// job (`messaging.rs`) before this is ever invoked -- this method only
    /// enforces the one *per-recipient* quota, `inbox_rows_max`
    /// (requirement 7), since that's the one whose outcome depends on
    /// which recipient is being resolved.
    #[allow(clippy::too_many_arguments)]
    pub async fn msg_send(
        &self,
        sender: Tenant,
        sender_synthetic: Option<String>,
        to: Vec<String>,
        thread_id: Option<String>,
        body: String,
        data_json: Option<String>,
        dedupe_key: Option<String>,
        inbox_rows_max: i64,
        // PRD-mcphost-agent-consent requirement 6.
        urgent: bool,
        urgent_per_day: i64,
    ) -> Result<SendOutcome, AppError> {
        let now_ms = crate::state::now_unix_ms();
        let now = now_rfc3339();
        self.with_conn(move |conn| {
            conn.execute("BEGIN IMMEDIATE", []).map_err(AppError::from)?;
            let outcome: Result<SendOutcome, AppError> = (|| {
                if let Some(key) = &dedupe_key
                    && let Some(hit) = Self::dedupe_hit(conn, sender.id, key, now_ms)?
                {
                    return Ok(hit);
                }

                let (tid, existing_participants) = match &thread_id {
                    Some(t) => {
                        let is_participant: bool = conn
                            .prepare("SELECT 1 FROM thread_participants WHERE thread_id = ?1 AND tenant_id = ?2")?
                            .exists(params![t, sender.id])?;
                        if !is_participant {
                            return Err(AppError::thread_not_found());
                        }
                        let existing = Self::thread_participant_ids(conn, t)?;
                        (t.clone(), existing)
                    }
                    None => {
                        let new_id = crate::state::new_ulid();
                        conn.execute(
                            "INSERT INTO threads (id, created_by, created_at, created_unix_ms) VALUES (?1, ?2, ?3, ?4)",
                            params![new_id, sender.id, now, now_ms],
                        )?;
                        conn.execute(
                            "INSERT INTO thread_participants (thread_id, tenant_id, joined_at) VALUES (?1, ?2, ?3)",
                            params![new_id, sender.id, now],
                        )?;
                        (new_id, vec![sender.id])
                    }
                };

                let mut delivered_to = Vec::new();
                let mut refused: Vec<(String, &'static str)> = Vec::new();
                let mut deliver_ids = Vec::new();
                for addr in &to {
                    match Self::resolve_message_recipient(conn, addr, sender.id, now_ms)? {
                        RecipientResolution::Refused(code) => refused.push((addr.clone(), code)),
                        RecipientResolution::Ok(t) => {
                            // requirement 6: urgent_per_day is a
                            // sender-per-recipient whole-call quota -- a
                            // recipient over it fails the whole send (AC6:
                            // "the fourth ... is not stored"), unlike
                            // inbox_rows_max below, which only refuses that
                            // one recipient. Sliding 24h window, same
                            // convention as msgs_per_hour's sliding hour.
                            if urgent {
                                let sent_today: i64 = conn.query_row(
                                    "SELECT COUNT(*) FROM messages m \
                                     JOIN message_receipts r ON r.message_id = m.id \
                                     WHERE m.from_tenant_id = ?1 AND r.tenant_id = ?2 \
                                       AND m.urgent = 1 AND m.created_unix_ms >= ?3",
                                    params![sender.id, t.id, now_ms - 86_400_000],
                                    |r| r.get(0),
                                )?;
                                if sent_today >= urgent_per_day {
                                    return Err(AppError::msg_quota_exceeded("urgent_per_day", urgent_per_day));
                                }
                            }
                            let rows: i64 = conn.query_row(
                                "SELECT COUNT(*) FROM message_receipts WHERE tenant_id = ?1",
                                params![t.id],
                                |r| r.get(0),
                            )?;
                            if rows >= inbox_rows_max {
                                refused.push((addr.clone(), "recipient_inbox_full"));
                                continue;
                            }
                            if !existing_participants.contains(&t.id) {
                                conn.execute(
                                    "INSERT OR IGNORE INTO thread_participants (thread_id, tenant_id, joined_at) \
                                     VALUES (?1, ?2, ?3)",
                                    params![tid, t.id, now],
                                )?;
                            }
                            delivered_to.push(t.namespace.clone());
                            deliver_ids.push(t.id);
                        }
                    }
                }

                let (message_id, seq) = Self::insert_message(
                    conn, &tid, sender.id, &sender.namespace, &sender_synthetic,
                    sender.source_class.as_deref().unwrap_or("external"),
                    &body, &data_json, None, &dedupe_key, now_ms, &now, &deliver_ids, &refused, urgent,
                )?;

                Ok(SendOutcome {
                    message_id,
                    thread_id: tid,
                    seq,
                    delivered_to,
                    delivered_tenant_ids: deliver_ids,
                    refused,
                    deduped: false,
                    created_at: now.clone(),
                })
            })();
            match &outcome {
                Ok(_) => conn.execute("COMMIT", []).map(|_| ()).map_err(AppError::from)?,
                Err(_) => {
                    let _ = conn.execute("ROLLBACK", []);
                }
            }
            outcome
        })
        .await
    }

    /// `host.msg.reply` (requirement 3 / AC2, AC3): caller must already be
    /// a thread participant (`thread_not_found` otherwise, byte-identical
    /// to a nonexistent thread id -- AC3); delivers to every OTHER current
    /// participant, live-checking each one's block/contact_policy state
    /// the same way [`Self::resolve_message_recipient`] does for a fresh
    /// `send`, skipping (and listing in `refused`) any that currently
    /// refuse the replier rather than failing the whole reply.
    #[allow(clippy::too_many_arguments)]
    pub async fn msg_reply(
        &self,
        sender: Tenant,
        sender_synthetic: Option<String>,
        thread_id: String,
        body: String,
        data_json: Option<String>,
        in_reply_to: Option<String>,
        dedupe_key: Option<String>,
        inbox_rows_max: i64,
    ) -> Result<SendOutcome, AppError> {
        let now_ms = crate::state::now_unix_ms();
        let now = now_rfc3339();
        self.with_conn(move |conn| {
            conn.execute("BEGIN IMMEDIATE", []).map_err(AppError::from)?;
            let outcome: Result<SendOutcome, AppError> = (|| {
                if let Some(key) = &dedupe_key
                    && let Some(hit) = Self::dedupe_hit(conn, sender.id, key, now_ms)?
                {
                    return Ok(hit);
                }
                let is_participant: bool = conn
                    .prepare("SELECT 1 FROM thread_participants WHERE thread_id = ?1 AND tenant_id = ?2")?
                    .exists(params![thread_id, sender.id])?;
                if !is_participant {
                    return Err(AppError::thread_not_found());
                }
                let other_ids: Vec<i64> = Self::thread_participant_ids(conn, &thread_id)?
                    .into_iter()
                    .filter(|id| *id != sender.id)
                    .collect();

                let mut delivered_to = Vec::new();
                let mut refused: Vec<(String, &'static str)> = Vec::new();
                let mut deliver_ids = Vec::new();
                for other_id in other_ids {
                    let sql = format!("SELECT {TENANT_COLUMNS} FROM tenants t WHERE t.id = ?1");
                    let Some(t) = conn
                        .query_row(&sql, params![other_id], tenant_from_row)
                        .optional()?
                    else {
                        continue; // participant tenant was deleted; nothing to refuse or deliver to
                    };
                    // PRD-mcphost-agent-consent requirement 4: same shared
                    // `consent::check` `resolve_message_recipient` calls
                    // above, so a reply's blocked/policy handling can't
                    // drift from a fresh send's.
                    let policy = Self::query_agent_profile(conn, t.id)?.contact_policy;
                    match crate::consent::check(conn, sender.id, t.id, &policy, now_ms)? {
                        crate::consent::SendCheck::Allow => {}
                        crate::consent::SendCheck::Refuse(code) => {
                            refused.push((t.namespace.clone(), code));
                            continue;
                        }
                    }
                    let rows: i64 = conn.query_row(
                        "SELECT COUNT(*) FROM message_receipts WHERE tenant_id = ?1",
                        params![t.id],
                        |r| r.get(0),
                    )?;
                    if rows >= inbox_rows_max {
                        refused.push((t.namespace.clone(), "recipient_inbox_full"));
                        continue;
                    }
                    delivered_to.push(t.namespace.clone());
                    deliver_ids.push(t.id);
                }

                let (message_id, seq) = Self::insert_message(
                    conn, &thread_id, sender.id, &sender.namespace, &sender_synthetic,
                    sender.source_class.as_deref().unwrap_or("external"),
                    &body, &data_json, in_reply_to.as_deref(), &dedupe_key, now_ms, &now, &deliver_ids,
                    &refused, false,
                )?;

                Ok(SendOutcome {
                    message_id,
                    thread_id,
                    seq,
                    delivered_to,
                    delivered_tenant_ids: deliver_ids,
                    refused,
                    deduped: false,
                    created_at: now.clone(),
                })
            })();
            match &outcome {
                Ok(_) => conn.execute("COMMIT", []).map(|_| ()).map_err(AppError::from)?,
                Err(_) => {
                    let _ = conn.execute("ROLLBACK", []);
                }
            }
            outcome
        })
        .await
    }

    /// Shared by [`Self::msg_send`]/[`Self::msg_reply`]: a fresh resend
    /// within [`MSG_DEDUPE_WINDOW_MS`] returns the original outcome
    /// (`Some`); a stale one frees the key (nulls that row's `dedupe_key`)
    /// and returns `None` so the caller proceeds to store a new message;
    /// no prior use at all is also `None`.
    ///
    /// `refused` is rebuilt from `messages.refused_json` (advisory finding
    /// `dedupe-resend-refused-list-not-reconstructed`, PRD-mcphost-agent-inbox
    /// gate review) -- before this, a cache hit hardcoded `refused: vec![]`,
    /// so a resend of an originally-mixed delivered/refused send silently
    /// reported everyone delivered. `refused_json` is written once, at
    /// [`Self::insert_message`] time, so this replays the *original* send's
    /// outcome rather than re-deriving current state (e.g. a block added
    /// since then) -- consistent with "returns the original `message_id`"
    /// (requirement 8) applying to the whole outcome, not just the id.
    fn dedupe_hit(
        conn: &Connection,
        sender_id: i64,
        key: &str,
        now_ms: i64,
    ) -> Result<Option<SendOutcome>, AppError> {
        // (message_id, thread_id, seq, created_unix_ms, refused_json, created_at)
        type DedupeRow = (String, String, i64, i64, Option<String>, String);
        let existing: Option<DedupeRow> = conn
            .query_row(
                "SELECT id, thread_id, seq, created_unix_ms, refused_json, created_at FROM messages \
                 WHERE from_tenant_id = ?1 AND dedupe_key = ?2",
                params![sender_id, key],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .optional()?;
        let Some((message_id, thread_id, seq, created_unix_ms, refused_json, created_at)) = existing
        else {
            return Ok(None);
        };
        if now_ms - created_unix_ms < MSG_DEDUPE_WINDOW_MS {
            // PRD-mcphost-agent-wake: a resend within the dedupe window
            // still needs the original recipients' tenant ids, not just
            // their namespaces, for `fire_message_triggers`' own lookup --
            // the trigger-level `(trigger_id, message_id)` dedupe in
            // `event_dedupe` (requirement 3) is what actually prevents a
            // second run from a resend reaching here, not this layer.
            let delivered: Vec<(String, i64)> = {
                let mut stmt = conn.prepare(
                    "SELECT t.namespace, t.id FROM message_receipts r JOIN tenants t ON t.id = r.tenant_id \
                     WHERE r.message_id = ?1",
                )?;
                stmt.query_map(params![message_id], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            };
            let delivered_to = delivered.iter().map(|(ns, _)| ns.clone()).collect();
            let delivered_tenant_ids = delivered.iter().map(|(_, id)| *id).collect();
            let refused: Vec<(String, &'static str)> = refused_json
                .and_then(|s| serde_json::from_str::<Vec<(String, String)>>(&s).ok())
                .unwrap_or_default()
                .into_iter()
                .map(|(address, code)| (address, code_to_static(&code)))
                .collect();
            return Ok(Some(SendOutcome {
                message_id,
                thread_id,
                seq,
                delivered_to,
                delivered_tenant_ids,
                refused,
                deduped: true,
                created_at,
            }));
        }
        conn.execute("UPDATE messages SET dedupe_key = NULL WHERE id = ?1", params![message_id])?;
        Ok(None)
    }

    fn thread_participant_ids(conn: &Connection, thread_id: &str) -> Result<Vec<i64>, AppError> {
        let mut stmt = conn.prepare("SELECT tenant_id FROM thread_participants WHERE thread_id = ?1")?;
        let ids = stmt
            .query_map(params![thread_id], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        Ok(ids)
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_message(
        conn: &Connection,
        thread_id: &str,
        from_tenant_id: i64,
        from_address: &str,
        synthetic: &Option<String>,
        source_class: &str,
        body: &str,
        data_json: &Option<String>,
        in_reply_to: Option<&str>,
        dedupe_key: &Option<String>,
        now_ms: i64,
        now: &str,
        deliver_ids: &[i64],
        refused: &[(String, &'static str)],
        // PRD-mcphost-agent-consent requirement 6: `host.msg.reply` always
        // passes `false` -- only `host.msg.send` exposes `urgent`.
        urgent: bool,
    ) -> Result<(String, i64), AppError> {
        let seq: i64 = conn.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM messages WHERE thread_id = ?1",
            params![thread_id],
            |r| r.get(0),
        )?;
        let message_id = crate::state::new_ulid();
        // Advisory finding `dedupe-resend-refused-list-not-reconstructed`:
        // persisted so a later `dedupe_hit` on this row's `dedupe_key` can
        // replay the original `refused` list instead of losing it.
        let refused_json: Option<String> = if refused.is_empty() {
            None
        } else {
            Some(serde_json::to_string(refused).map_err(|e| AppError::Internal(e.to_string()))?)
        };
        conn.execute(
            "INSERT INTO messages \
                 (id, thread_id, seq, from_tenant_id, from_address, body, data_json, in_reply_to, \
                  dedupe_key, synthetic, source_class, created_at, created_unix_ms, refused_json, urgent) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                message_id, thread_id, seq, from_tenant_id, from_address, body, data_json, in_reply_to,
                dedupe_key, synthetic, source_class, now, now_ms, refused_json, urgent
            ],
        )?;
        for recipient_id in deliver_ids {
            conn.execute(
                "INSERT OR IGNORE INTO message_receipts (message_id, tenant_id, read_at) VALUES (?1, ?2, NULL)",
                params![message_id, recipient_id],
            )?;
        }
        Ok((message_id, seq))
    }

    /// Requirement 7: how many messages `tenant_id` has sent in the sliding
    /// window ending now, for `msgs_per_hour` (`messaging.rs` passes
    /// `now_ms - 3_600_000`).
    pub async fn count_messages_sent_since(&self, tenant_id: i64, since_unix_ms: i64) -> Result<i64, AppError> {
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM messages WHERE from_tenant_id = ?1 AND created_unix_ms >= ?2",
                params![tenant_id, since_unix_ms],
                |r| r.get(0),
            )
            .map_err(AppError::from)
        })
        .await
    }

    /// `host.msg.inbox` (requirement 4 / AC6, AC13): every row this
    /// tenant has a [`message_receipts`] entry for (which excludes its own
    /// sends by construction -- a sender never gets a receipt row for its
    /// own message, see migration 0021's doc comment), ordered by
    /// `(created_unix_ms, id)`, optionally resuming after an opaque cursor
    /// and filtered to unread (requirement 5 / AC13).
    pub async fn msg_inbox(
        &self,
        tenant_id: i64,
        after: Option<(i64, String)>,
        limit: i64,
        unread_only: bool,
    ) -> Result<Vec<MessageRow>, AppError> {
        self.with_conn(move |conn| {
            let mut sql = String::from(
                "SELECT m.id, m.thread_id, m.seq, m.from_address, m.body, m.data_json, m.in_reply_to, \
                        m.synthetic, m.source_class, m.created_at, m.created_unix_ms, r.read_at, m.urgent \
                 FROM message_receipts r JOIN messages m ON m.id = r.message_id \
                 LEFT JOIN muted mu ON mu.tenant_id = r.tenant_id AND mu.muted_tenant_id = m.from_tenant_id \
                 WHERE r.tenant_id = ?1",
            );
            let mut sql_params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(tenant_id)];
            if let Some((created_unix_ms, id)) = &after {
                sql.push_str(&format!(
                    " AND (m.created_unix_ms, m.id) > (?{}, ?{})",
                    sql_params.len() + 1,
                    sql_params.len() + 2
                ));
                sql_params.push(Box::new(*created_unix_ms));
                sql_params.push(Box::new(id.clone()));
            }
            if unread_only {
                // requirement 5 / 6 (AC5): a muted sender's message is
                // excluded from an unread_only read UNLESS it's urgent
                // (requirement 6: urgent bypasses the mute filter, never a
                // block or closed policy -- neither of which apply here,
                // both already keep the message out of `message_receipts`
                // entirely). `mu.tenant_id IS NULL` is "not muted" (the
                // LEFT JOIN found no matching mute row).
                sql.push_str(" AND r.read_at IS NULL AND (mu.tenant_id IS NULL OR m.urgent = 1)");
            }
            sql.push_str(&format!(" ORDER BY m.created_unix_ms, m.id LIMIT ?{}", sql_params.len() + 1));
            sql_params.push(Box::new(limit));

            let mut stmt = conn.prepare(&sql)?;
            let param_refs: Vec<&dyn rusqlite::ToSql> = sql_params.iter().map(|b| b.as_ref()).collect();
            let rows = stmt.query_map(param_refs.as_slice(), message_row_from_row)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
        .await
    }

    /// `host.msg.thread` (requirement 4): `None` when `tenant_id` is not
    /// (or no longer) a participant, mapped to `thread_not_found` by
    /// `messaging.rs`, byte-identical to a nonexistent thread id.
    pub async fn msg_thread(
        &self,
        tenant_id: i64,
        thread_id: String,
        after_seq: Option<i64>,
        limit: i64,
    ) -> Result<Option<Vec<MessageRow>>, AppError> {
        self.with_conn(move |conn| {
            let is_participant: bool = conn
                .prepare("SELECT 1 FROM thread_participants WHERE thread_id = ?1 AND tenant_id = ?2")?
                .exists(params![thread_id, tenant_id])?;
            if !is_participant {
                return Ok(None);
            }
            let mut sql = String::from(
                "SELECT m.id, m.thread_id, m.seq, m.from_address, m.body, m.data_json, m.in_reply_to, \
                        m.synthetic, m.source_class, m.created_at, m.created_unix_ms, r.read_at, m.urgent \
                 FROM messages m LEFT JOIN message_receipts r ON r.message_id = m.id AND r.tenant_id = ?1 \
                 WHERE m.thread_id = ?2",
            );
            let mut sql_params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(tenant_id), Box::new(thread_id.clone())];
            if let Some(seq) = after_seq {
                sql.push_str(&format!(" AND m.seq > ?{}", sql_params.len() + 1));
                sql_params.push(Box::new(seq));
            }
            sql.push_str(&format!(" ORDER BY m.seq LIMIT ?{}", sql_params.len() + 1));
            sql_params.push(Box::new(limit));

            let mut stmt = conn.prepare(&sql)?;
            let param_refs: Vec<&dyn rusqlite::ToSql> = sql_params.iter().map(|b| b.as_ref()).collect();
            let rows = stmt.query_map(param_refs.as_slice(), message_row_from_row)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(Some(out))
        })
        .await
    }

    /// Requirement 11 (AC14): how many tenants already participate in
    /// `thread_id` -- `messaging.rs` adds this to the new `to` list's
    /// length (minus any already-participating) before checking
    /// `recipients_per_msg_max` ("recipient cap counts total
    /// participants"). `0` for a nonexistent thread id; `msg_send` itself
    /// still returns `thread_not_found` if the caller isn't a participant.
    pub async fn count_thread_participants(&self, thread_id: String) -> Result<i64, AppError> {
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM thread_participants WHERE thread_id = ?1",
                params![thread_id],
                |r| r.get(0),
            )
            .map_err(AppError::from)
        })
        .await
    }

    /// `host.msg.ack` (requirement 5): sets `read_at` for every id in
    /// `message_ids` this tenant has a receipt for; ids that don't exist,
    /// or aren't addressed to this tenant, are silently skipped (not an
    /// error -- an ack is idempotent and best-effort per id). Returns how
    /// many rows changed.
    pub async fn msg_ack(&self, tenant_id: i64, message_ids: Vec<String>) -> Result<i64, AppError> {
        let now = now_rfc3339();
        self.with_conn(move |conn| {
            let mut changed = 0i64;
            for id in &message_ids {
                changed += conn.execute(
                    "UPDATE message_receipts SET read_at = ?1 \
                     WHERE message_id = ?2 AND tenant_id = ?3 AND read_at IS NULL",
                    params![now, id, tenant_id],
                )? as i64;
            }
            Ok(changed)
        })
        .await
    }

    /// Resolve `address` to a tenant id for `host.msg.block`/`unblock`
    /// (requirement 9), excluding disabled tenants same as
    /// [`Self::resolve_message_recipient`] (but with no block/policy
    /// checks of its own -- blocking is unconditional).
    pub async fn resolve_agent_address(&self, address: String) -> Result<Option<i64>, AppError> {
        self.with_conn(move |conn| {
            let id: Option<i64> = if let Some(h) = address.strip_prefix('@') {
                let handle = h.to_lowercase();
                conn.query_row(
                    "SELECT t.id FROM tenants t JOIN agent_profiles ap ON ap.tenant_id = t.id \
                     WHERE ap.handle = ?1 AND t.disabled = 0",
                    params![handle],
                    |r| r.get(0),
                )
                .optional()?
            } else {
                conn.query_row(
                    "SELECT id FROM tenants WHERE namespace = ?1 AND disabled = 0",
                    params![address],
                    |r| r.get(0),
                )
                .optional()?
            };
            Ok(id)
        })
        .await
    }

    /// `host.msg.block(address)` (requirement 9; PRD-mcphost-agent-consent
    /// requirement 7): idempotent -- blocking an already-blocked address is
    /// a no-op success. Also denies any `pending` request from the blocked
    /// party (so it can't sit there forever un-decided) and removes an
    /// existing accepted `contacts` pair (both direction rows) -- neither
    /// is ever visible to the blocked party: they just start getting
    /// `agent_not_found`/`contact_refused` like anyone blocked already
    /// does (`consent::check`), no notification of any kind.
    pub async fn msg_block(&self, tenant_id: i64, blocked_tenant_id: i64) -> Result<(), AppError> {
        let now = now_rfc3339();
        let now_ms = crate::state::now_unix_ms();
        self.with_conn(move |conn| {
            conn.execute("BEGIN IMMEDIATE", []).map_err(AppError::from)?;
            let result: Result<(), AppError> = (|| {
                conn.execute(
                    "INSERT OR IGNORE INTO blocks (tenant_id, blocked_tenant_id, created_at) VALUES (?1, ?2, ?3)",
                    params![tenant_id, blocked_tenant_id, now],
                )?;
                conn.execute(
                    "UPDATE contact_requests SET status = 'denied', decided_at = ?1, decided_unix_ms = ?2 \
                     WHERE from_tenant_id = ?3 AND to_tenant_id = ?4 AND status = 'pending'",
                    params![now, now_ms, blocked_tenant_id, tenant_id],
                )?;
                conn.execute(
                    "DELETE FROM contacts WHERE (tenant_id = ?1 AND contact_tenant_id = ?2) \
                        OR (tenant_id = ?2 AND contact_tenant_id = ?1)",
                    params![tenant_id, blocked_tenant_id],
                )?;
                Ok(())
            })();
            match &result {
                Ok(()) => conn.execute("COMMIT", []).map(|_| ()).map_err(AppError::from)?,
                Err(_) => {
                    let _ = conn.execute("ROLLBACK", []);
                }
            }
            result
        })
        .await
    }

    /// `host.msg.unblock(address)` (requirement 9): `true` if a block was
    /// actually removed.
    pub async fn msg_unblock(&self, tenant_id: i64, blocked_tenant_id: i64) -> Result<bool, AppError> {
        self.with_conn(move |conn| {
            let n = conn.execute(
                "DELETE FROM blocks WHERE tenant_id = ?1 AND blocked_tenant_id = ?2",
                params![tenant_id, blocked_tenant_id],
            )?;
            Ok(n > 0)
        })
        .await
    }

    // ---- consent (PRD-mcphost-agent-consent) -----------------------------

    /// Test/ops-only: backdate a `contact_requests` row's `created_unix_ms`
    /// and/or `decided_unix_ms` -- AC4's 7-day deny-cooldown test needs a
    /// way to simulate the window elapsing without a real 7-day wait, same
    /// "toggle internal state directly rather than waiting out a real
    /// window" convention as [`Self::set_query_only`] (AC14's unwritable-db
    /// simulation). `None` leaves that column unchanged.
    pub async fn test_backdate_contact_request(
        &self,
        request_id: String,
        created_unix_ms: Option<i64>,
        decided_unix_ms: Option<i64>,
    ) -> Result<(), AppError> {
        self.with_conn(move |conn| {
            if let Some(ms) = created_unix_ms {
                conn.execute(
                    "UPDATE contact_requests SET created_unix_ms = ?1 WHERE id = ?2",
                    params![ms, request_id],
                )?;
            }
            if let Some(ms) = decided_unix_ms {
                conn.execute(
                    "UPDATE contact_requests SET decided_unix_ms = ?1 WHERE id = ?2",
                    params![ms, request_id],
                )?;
            }
            Ok(())
        })
        .await
    }

    /// `host.agent.mute(address)` (requirement 5): unconditional, same
    /// idempotent-no-op shape as [`Self::msg_block`]'s own insert.
    pub async fn msg_mute(&self, tenant_id: i64, muted_tenant_id: i64) -> Result<(), AppError> {
        let now = now_rfc3339();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT OR IGNORE INTO muted (tenant_id, muted_tenant_id, created_at) VALUES (?1, ?2, ?3)",
                params![tenant_id, muted_tenant_id, now],
            )?;
            Ok(())
        })
        .await
    }

    /// `host.agent.unmute(address)` (requirement 5): `true` if a mute was
    /// actually removed.
    pub async fn msg_unmute(&self, tenant_id: i64, muted_tenant_id: i64) -> Result<bool, AppError> {
        self.with_conn(move |conn| {
            let n = conn.execute(
                "DELETE FROM muted WHERE tenant_id = ?1 AND muted_tenant_id = ?2",
                params![tenant_id, muted_tenant_id],
            )?;
            Ok(n > 0)
        })
        .await
    }

    /// Requirement 5 (AC5): the same `muted` predicate `msg_inbox`'s
    /// `LEFT JOIN muted` above already applies to unread-count filtering,
    /// exposed standalone for `messaging::fire_message_triggers` -- a
    /// muted sender's non-urgent message must not fire the recipient's
    /// message trigger either (reviewer-agent finding
    /// `ac5-mute-does-not-suppress-message-trigger-runs`: the trigger-fire
    /// path never consulted this table before, so a muted sender could
    /// still wake the recipient's bound tool).
    pub async fn is_muted(&self, recipient_id: i64, sender_id: i64) -> Result<bool, AppError> {
        self.with_conn(move |conn| {
            let muted: bool = conn
                .prepare("SELECT 1 FROM muted WHERE tenant_id = ?1 AND muted_tenant_id = ?2")?
                .exists(params![recipient_id, sender_id])?;
            Ok(muted)
        })
        .await
    }

    /// `host.agent.contact_request(address, note?)` (requirement 2 / AC1,
    /// AC2, AC4, AC7; requirement 9 / AC10; requirement 11's lazy expiry):
    /// resolves `to_address` the same way [`Self::resolve_agent_address`]
    /// does (address resolution carries no policy/block filtering of its
    /// own -- both are checked explicitly below so AC7's "byte-identical to
    /// a nonexistent address" is reachable for both "doesn't resolve" and
    /// "resolves but blocked me", the same `agent_not_found` code either
    /// way). Every refusal is returned as `Err`, not a variant, since
    /// there's nothing left for a caller to do with a `Result::Ok` that
    /// isn't itself a pending request (`consent.rs::contact_request` is a
    /// thin pass-through for exactly this reason).
    pub async fn contact_request(
        &self,
        from_tenant: Tenant,
        to_address: String,
        note: Option<String>,
        contact_requests_per_day: i64,
    ) -> Result<crate::consent::ExistingPendingRequest, AppError> {
        // PRD-mcphost-agent-mesh-ops requirement 4 / AC4: checked once
        // here rather than in each of this method's two call sites
        // (`consent::contact_request`'s tool wrapper and
        // `consent::contacts_import`'s per-address loop, which calls this
        // method directly) -- "one place a send is allowed or refused",
        // same posture as `messaging::send`/`reply`'s own top-of-function
        // check.
        if from_tenant.mesh_frozen_at.is_some() {
            return Err(AppError::mesh_frozen());
        }
        let now_ms = crate::state::now_unix_ms();
        let now = now_rfc3339();
        self.with_conn(move |conn| {
            conn.execute("BEGIN IMMEDIATE", []).map_err(AppError::from)?;
            let outcome: Result<crate::consent::ExistingPendingRequest, AppError> = (|| {
                let to_id: Option<i64> = if let Some(h) = to_address.strip_prefix('@') {
                    let handle = h.to_lowercase();
                    conn.query_row(
                        "SELECT t.id FROM tenants t JOIN agent_profiles ap ON ap.tenant_id = t.id \
                         WHERE ap.handle = ?1 AND t.disabled = 0",
                        params![handle],
                        |r| r.get(0),
                    )
                    .optional()?
                } else {
                    conn.query_row(
                        "SELECT id FROM tenants WHERE namespace = ?1 AND disabled = 0",
                        params![to_address],
                        |r| r.get(0),
                    )
                    .optional()?
                };
                let Some(to_id) = to_id else { return Err(AppError::agent_not_found()) };
                // AC7: blocked-by-recipient reads identically to a
                // nonexistent address, same convention `consent::check`
                // already applies to `host.msg.send`.
                if crate::consent::is_blocked(conn, to_id, from_tenant.id)? {
                    return Err(AppError::agent_not_found());
                }
                if crate::consent::has_accepted_contact(conn, from_tenant.id, to_id)? {
                    return Err(AppError::not_needed());
                }
                let policy = Self::query_agent_profile(conn, to_id)?.contact_policy;
                match policy.as_str() {
                    "open" => return Err(AppError::not_needed()),
                    "closed" => return Err(AppError::contact_refused()),
                    _ => {} // "contacts"
                }
                match crate::consent::classify_existing_request(conn, from_tenant.id, to_id, now_ms)? {
                    crate::consent::ExistingRequestState::Pending(existing) => return Ok(existing),
                    crate::consent::ExistingRequestState::Cooldown => {
                        return Err(AppError::contact_pending());
                    }
                    crate::consent::ExistingRequestState::None
                    | crate::consent::ExistingRequestState::Stale => {}
                }
                // requirement 2: contact_requests_per_day, sliding 24h,
                // whole-call (sender-wide, not per-recipient -- unlike
                // urgent_per_day, nothing in the PRD scopes this one to a
                // pair).
                let sent_today: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM contact_requests WHERE from_tenant_id = ?1 AND created_unix_ms >= ?2",
                    params![from_tenant.id, now_ms - 86_400_000],
                    |r| r.get(0),
                )?;
                if sent_today >= contact_requests_per_day {
                    return Err(AppError::msg_quota_exceeded(
                        "contact_requests_per_day",
                        contact_requests_per_day,
                    ));
                }
                let id = crate::state::new_ulid();
                // requirement 3's cooldown-expiry note: `UNIQUE(from_tenant_id,
                // to_tenant_id)` means a denied-and-stale (or expired) row
                // is replaced in place, not inserted as a second row.
                conn.execute(
                    "INSERT INTO contact_requests \
                         (id, from_tenant_id, to_tenant_id, note, status, created_at, created_unix_ms) \
                     VALUES (?1, ?2, ?3, ?4, 'pending', ?5, ?6) \
                     ON CONFLICT(from_tenant_id, to_tenant_id) DO UPDATE SET \
                         id = excluded.id, note = excluded.note, status = 'pending', \
                         created_at = excluded.created_at, created_unix_ms = excluded.created_unix_ms, \
                         decided_at = NULL, decided_unix_ms = NULL",
                    params![id, from_tenant.id, to_id, note, now, now_ms],
                )?;
                // requirement 9 (P1, AC10): a system message in the
                // recipient's inbox via the EXISTING `Db::msg_inbox` query
                // path -- no changes needed there.
                Self::insert_contact_request_notice(conn, to_id, &id, &now, now_ms)?;
                Ok(crate::consent::ExistingPendingRequest {
                    request_id: id,
                    note,
                    created_at: now,
                })
            })();
            match &outcome {
                Ok(_) => conn.execute("COMMIT", []).map(|_| ()).map_err(AppError::from)?,
                Err(_) => {
                    let _ = conn.execute("ROLLBACK", []);
                }
            }
            outcome
        })
        .await
    }

    /// Requirement 9 (P1, AC10): a synthetic "host" system message
    /// announcing a new contact request, appended to a per-recipient
    /// synthetic thread (`host-inbox-<to_tenant_id>`, created on first use)
    /// -- reuses `messages`/`message_receipts`/`thread_participants`
    /// storage rather than a parallel path, so it surfaces via the
    /// existing `Db::msg_inbox` query with no changes there. `from_tenant_id`
    /// is `NULL` (nothing real tenant sent this): `from_address: "host"` is
    /// hardcoded, the same "denormalized address survives a null tenant"
    /// shape migration 0021 already gives a deleted sender's messages.
    fn insert_contact_request_notice(
        conn: &Connection,
        to_tenant_id: i64,
        request_id: &str,
        now: &str,
        now_ms: i64,
    ) -> Result<(), AppError> {
        let thread_id = format!("host-inbox-{to_tenant_id}");
        let thread_exists: bool = conn
            .prepare("SELECT 1 FROM threads WHERE id = ?1")?
            .exists(params![thread_id])?;
        if !thread_exists {
            conn.execute(
                "INSERT INTO threads (id, created_by, created_at, created_unix_ms) VALUES (?1, NULL, ?2, ?3)",
                params![thread_id, now, now_ms],
            )?;
        }
        conn.execute(
            "INSERT OR IGNORE INTO thread_participants (thread_id, tenant_id, joined_at) VALUES (?1, ?2, ?3)",
            params![thread_id, to_tenant_id, now],
        )?;
        let seq: i64 = conn.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM messages WHERE thread_id = ?1",
            params![thread_id],
            |r| r.get(0),
        )?;
        let message_id = crate::state::new_ulid();
        let data_json = json!({"kind": "contact_request", "request_id": request_id}).to_string();
        conn.execute(
            "INSERT INTO messages \
                 (id, thread_id, seq, from_tenant_id, from_address, body, data_json, in_reply_to, \
                  dedupe_key, synthetic, source_class, created_at, created_unix_ms, refused_json, urgent) \
             VALUES (?1, ?2, ?3, NULL, 'host', ?4, ?5, NULL, NULL, NULL, 'host', ?6, ?7, NULL, 0)",
            params![
                message_id,
                thread_id,
                seq,
                "You have a new contact request.",
                data_json,
                now,
                now_ms
            ],
        )?;
        conn.execute(
            "INSERT INTO message_receipts (message_id, tenant_id, read_at) VALUES (?1, ?2, NULL)",
            params![message_id, to_tenant_id],
        )?;
        Ok(())
    }

    /// `host.agent.contacts(status?)` (requirement 3 / AC2, AC3): the
    /// caller's accepted contacts, plus every pending/decided request in
    /// either direction, `status` optionally narrowing the two request
    /// lists (never the `contacts` list itself -- an accepted pair has no
    /// other status).
    pub async fn agent_contacts(
        &self,
        tenant_id: i64,
        status: Option<String>,
    ) -> Result<ContactsView, AppError> {
        self.with_conn(move |conn| {
            let mut contacts_stmt = conn.prepare(
                "SELECT t.namespace, c.accepted_at FROM contacts c JOIN tenants t ON t.id = c.contact_tenant_id \
                 WHERE c.tenant_id = ?1 ORDER BY c.accepted_at",
            )?;
            let contacts = contacts_stmt
                .query_map(params![tenant_id], |r| {
                    Ok(ContactRow {
                        address: r.get(0)?,
                        accepted_at: r.get(1)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            let mut incoming_sql = String::from(
                "SELECT cr.id, t.namespace, cr.note, cr.status, cr.created_at, cr.decided_at \
                 FROM contact_requests cr JOIN tenants t ON t.id = cr.from_tenant_id \
                 WHERE cr.to_tenant_id = ?1",
            );
            let mut outgoing_sql = String::from(
                "SELECT cr.id, t.namespace, cr.note, cr.status, cr.created_at, cr.decided_at \
                 FROM contact_requests cr JOIN tenants t ON t.id = cr.to_tenant_id \
                 WHERE cr.from_tenant_id = ?1",
            );
            if status.is_some() {
                incoming_sql.push_str(" AND cr.status = ?2");
                outgoing_sql.push_str(" AND cr.status = ?2");
            }
            incoming_sql.push_str(" ORDER BY cr.created_at");
            outgoing_sql.push_str(" ORDER BY cr.created_at");

            let read_requests = |sql: &str| -> Result<Vec<ContactRequestRow>, AppError> {
                let mut stmt = conn.prepare(sql)?;
                let rows = if let Some(s) = &status {
                    stmt.query_map(params![tenant_id, s], contact_request_row_from_row)?
                        .collect::<rusqlite::Result<Vec<_>>>()?
                } else {
                    stmt.query_map(params![tenant_id], contact_request_row_from_row)?
                        .collect::<rusqlite::Result<Vec<_>>>()?
                };
                Ok(rows)
            };
            let incoming = read_requests(&incoming_sql)?;
            let outgoing = read_requests(&outgoing_sql)?;

            Ok(ContactsView { contacts, incoming, outgoing })
        })
        .await
    }

    /// `host.agent.contact_accept(request_id)` (requirement 3 / AC3): only
    /// the request's own recipient may accept it; an unknown id, one
    /// addressed to someone else, or one no longer `pending` all collapse
    /// to [`AppError::contact_request_not_found`] (same "unknown vs
    /// hidden" convention as [`AppError::thread_not_found`]) -- inserts the
    /// symmetric `contacts` pair (requirement 3: "so `host.agent.contacts()`
    /// shows the pair from both sides") and flips `contact_requests.status`
    /// to `accepted` in the same transaction.
    pub async fn contact_accept(
        &self,
        tenant_id: i64,
        request_id: String,
    ) -> Result<ContactDecision, AppError> {
        self.contact_decide(tenant_id, request_id, true).await
    }

    /// `host.agent.contact_deny(request_id)` (requirement 3 / AC4): same
    /// authorization/idempotency shape as [`Self::contact_accept`], minus
    /// the `contacts` insert -- the requester's subsequent sends/requests
    /// read `contact_pending` for `consent::DENY_COOLDOWN_MS` from
    /// `decided_at` (`consent::classify_existing_request`).
    pub async fn contact_deny(
        &self,
        tenant_id: i64,
        request_id: String,
    ) -> Result<ContactDecision, AppError> {
        self.contact_decide(tenant_id, request_id, false).await
    }

    async fn contact_decide(
        &self,
        tenant_id: i64,
        request_id: String,
        accept: bool,
    ) -> Result<ContactDecision, AppError> {
        let now = now_rfc3339();
        let now_ms = crate::state::now_unix_ms();
        self.with_conn(move |conn| {
            conn.execute("BEGIN IMMEDIATE", []).map_err(AppError::from)?;
            let outcome: Result<ContactDecision, AppError> = (|| {
                type Row = (i64, i64, String, i64);
                let row: Option<Row> = conn
                    .query_row(
                        "SELECT from_tenant_id, to_tenant_id, status, created_unix_ms \
                         FROM contact_requests WHERE id = ?1",
                        params![request_id],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                    )
                    .optional()?;
                let Some((from_id, to_id, status, created_unix_ms)) = row else {
                    return Err(AppError::contact_request_not_found());
                };
                if to_id != tenant_id || status != "pending" {
                    return Err(AppError::contact_request_not_found());
                }
                // Requirement 11 / AC10 gap: `status` alone can lag reality
                // -- expiry is applied lazily by
                // `consent::classify_existing_request`, which only runs on
                // a `contact_request`/`check` read, not on accept/deny. A
                // request past `consent::REQUEST_EXPIRY_MS` that nothing
                // has re-read yet still shows `status = 'pending'` here; on
                // its own that let a stale accept/deny succeed against a
                // request that should already read as gone. Apply the same
                // expiry rule contact_decide's own read establishes before
                // deciding, flipping the row the same way
                // classify_existing_request would.
                if now_ms - created_unix_ms > crate::consent::REQUEST_EXPIRY_MS {
                    conn.execute(
                        "UPDATE contact_requests SET status = 'expired' WHERE id = ?1",
                        params![request_id],
                    )?;
                    return Err(AppError::contact_request_not_found());
                }
                let new_status = if accept { "accepted" } else { "denied" };
                conn.execute(
                    "UPDATE contact_requests SET status = ?1, decided_at = ?2, decided_unix_ms = ?3 \
                     WHERE id = ?4",
                    params![new_status, now, now_ms, request_id],
                )?;
                if accept {
                    conn.execute(
                        "INSERT OR REPLACE INTO contacts (tenant_id, contact_tenant_id, accepted_at) \
                         VALUES (?1, ?2, ?3)",
                        params![from_id, to_id, now],
                    )?;
                    conn.execute(
                        "INSERT OR REPLACE INTO contacts (tenant_id, contact_tenant_id, accepted_at) \
                         VALUES (?1, ?2, ?3)",
                        params![to_id, from_id, now],
                    )?;
                }
                let other_address: String = conn.query_row(
                    "SELECT namespace FROM tenants WHERE id = ?1",
                    params![from_id],
                    |r| r.get(0),
                )?;
                Ok(ContactDecision {
                    request_id: request_id.clone(),
                    other_address,
                    decided_at: now.clone(),
                })
            })();
            match &outcome {
                Ok(_) => conn.execute("COMMIT", []).map(|_| ()).map_err(AppError::from)?,
                Err(_) => {
                    let _ = conn.execute("ROLLBACK", []);
                }
            }
            outcome
        })
        .await
    }

    // ---- retention (PRD-mcphost-data-retention) ---------------------

    /// Requirement 1: (re-)seeds `retention_policy` from
    /// `retention::POLICY_TABLES`' env vars, called once at every `serve`
    /// start (after `migrate()`) so an operator's changed
    /// `$MCPHOST_RETENTION_*_DAYS` takes effect on restart without a
    /// migration. `INSERT ... ON CONFLICT DO UPDATE` rather than `INSERT
    /// OR IGNORE`: unlike a migration's one-time seed, this must overwrite
    /// a stale value every time, not just fill a gap the first time.
    pub async fn seed_retention_policy_from_env(&self) -> Result<(), AppError> {
        let now = crate::state::now_unix();
        let windows: Vec<(&'static str, i64)> = crate::retention::POLICY_TABLES
            .iter()
            .map(|t| (t.name, crate::retention::days_from_env(t.env_var, t.default_days)))
            .collect();
        self.with_conn(move |conn| {
            for (name, days) in &windows {
                conn.execute(
                    "INSERT INTO retention_policy (table_name, days, updated_unix) \
                     VALUES (?1, ?2, ?3) \
                     ON CONFLICT(table_name) DO UPDATE SET days = excluded.days, \
                     updated_unix = excluded.updated_unix",
                    params![name, days, now],
                )?;
            }
            Ok(())
        })
        .await
    }

    // ---- mesh ops (PRD-mcphost-agent-mesh-ops) ---------------------------

    /// `/healthz`'s `mesh.messages_24h`/`mesh.posts_24h`/`mesh.frozen_tenants`
    /// (requirement 6 / AC8): three cheap counts on a fixed 24h window --
    /// independent of `admin.mesh.stats`' own caller-chosen `window`.
    pub async fn mesh_healthz_counts(&self) -> Result<(i64, i64, i64), AppError> {
        let since_ms = crate::state::now_unix_ms() - 86_400_000;
        self.with_conn(move |conn| {
            let messages_24h: i64 = conn.query_row(
                "SELECT COUNT(*) FROM messages WHERE created_unix_ms >= ?1",
                params![since_ms],
                |r| r.get(0),
            )?;
            let posts_24h: i64 = conn.query_row(
                "SELECT COUNT(*) FROM channel_posts WHERE created_unix_ms >= ?1",
                params![since_ms],
                |r| r.get(0),
            )?;
            let frozen_tenants: i64 = conn.query_row(
                "SELECT COUNT(*) FROM tenants WHERE mesh_frozen_at IS NOT NULL",
                [],
                |r| r.get(0),
            )?;
            Ok((messages_24h, posts_24h, frozen_tenants))
        })
        .await
    }

    /// P1 requirement 5 (AC8): every configured table's retention window,
    /// table name ascending -- `host.usage` lists these unfiltered by
    /// tenant, since retention is a host-wide policy, not a per-tenant one.
    pub async fn retention_windows(&self) -> Result<Vec<(String, i64)>, AppError> {
        self.with_conn(|conn| {
            let mut stmt =
                conn.prepare("SELECT table_name, days FROM retention_policy ORDER BY table_name")?;
            let rows = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// Test-only: override one table's retention window directly, in
    /// place of setting the real `$MCPHOST_RETENTION_<TABLE>_DAYS`
    /// process env var, which would race across the several test
    /// functions that share one test binary (same "flip an internal knob
    /// for a test" rationale `TestServer::start_with_signup_rate_limit`'s
    /// own doc comment gives for the analogous
    /// `$MCPHOST_SIGNUP_RATE_LIMIT_PER_HOUR` case).
    pub async fn set_retention_days_for_test(
        &self,
        table: String,
        days: i64,
    ) -> Result<(), AppError> {
        let now = crate::state::now_unix();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO retention_policy (table_name, days, updated_unix) \
                 VALUES (?1, ?2, ?3) \
                 ON CONFLICT(table_name) DO UPDATE SET days = excluded.days, \
                 updated_unix = excluded.updated_unix",
                params![table, days, now],
            )?;
            Ok(())
        })
        .await
    }

    /// `admin.mesh.freeze(tenant, reason)` (requirement 4 / AC4, AC5): sets
    /// `tenants.mesh_frozen_at` and writes one `admin_events` row.
    /// `None` if no such tenant exists.
    pub async fn mesh_freeze(
        &self,
        tenant_ns: String,
        reason: Option<String>,
    ) -> Result<Option<Tenant>, AppError> {
        let now = now_rfc3339();
        self.with_conn(move |conn| {
            let Some(tenant) = Self::query_tenant_by_namespace(conn, &tenant_ns)? else {
                return Ok(None);
            };
            conn.execute(
                "UPDATE tenants SET mesh_frozen_at = ?1 WHERE id = ?2",
                params![now, tenant.id],
            )?;
            let detail = json!({"reason": reason}).to_string();
            conn.execute(
                "INSERT INTO admin_events (ts, action, tenant, detail) VALUES (?1, ?2, ?3, ?4)",
                params![now_rfc3339(), "mesh.freeze", tenant.namespace, detail],
            )?;
            let mut frozen = tenant;
            frozen.mesh_frozen_at = Some(now.clone());
            Ok(Some(frozen))
        })
        .await
    }

    /// `admin.mesh.unfreeze(tenant)` (requirement 4 / AC5).
    pub async fn mesh_unfreeze(&self, tenant_ns: String) -> Result<Option<Tenant>, AppError> {
        self.with_conn(move |conn| {
            let Some(tenant) = Self::query_tenant_by_namespace(conn, &tenant_ns)? else {
                return Ok(None);
            };
            conn.execute(
                "UPDATE tenants SET mesh_frozen_at = NULL WHERE id = ?1",
                params![tenant.id],
            )?;
            conn.execute(
                "INSERT INTO admin_events (ts, action, tenant, detail) VALUES (?1, ?2, ?3, ?4)",
                params![now_rfc3339(), "mesh.unfreeze", tenant.namespace, "{}"],
            )?;
            let mut unfrozen = tenant;
            unfrozen.mesh_frozen_at = None;
            Ok(Some(unfrozen))
        })
        .await
    }

    /// `admin.mesh.thread`'s own audit write (requirement 3 / AC3): every
    /// call writes exactly one `admin_events` row -- `target` (a thread or
    /// channel id) carried in the `tenant` column, same "generic target"
    /// reuse [`Self::release_handle`] already established for a non-tenant
    /// target.
    pub async fn record_mesh_thread_read(
        &self,
        target: String,
        reason: Option<String>,
    ) -> Result<(), AppError> {
        let detail = json!({"reason": reason}).to_string();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO admin_events (ts, action, tenant, detail) VALUES (?1, ?2, ?3, ?4)",
                params![now_rfc3339(), "mesh.thread_read", target, detail],
            )?;
            Ok(())
        })
        .await
    }

    /// Runs one full prune cycle -- the nightly scheduler and every AC1-4
    /// test funnel through this one method (a later admin-triggered
    /// on-demand cycle and `/healthz`/`admin.usage` reads of its outcome
    /// are additional callers layered on top of this same method) -- and
    /// journals its outcome to `prune_log` regardless of success. The
    /// actual batched deletes run on `retention::prune_sync`'s own
    /// dedicated connection (see that function's doc comment for why);
    /// this method only does the quick before/after bookkeeping on the
    /// shared connection.
    pub async fn prune_once(&self) -> Result<PruneReport, AppError> {
        let windows = self.retention_windows().await?;
        let path = self.path.clone();
        let started = crate::state::now_unix();
        let outcome = tokio::task::spawn_blocking(move || {
            crate::retention::prune_sync(&path, &windows, started)
        })
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
        let finished = crate::state::now_unix();
        let ok = outcome.error.is_none();
        let deleted_json = serde_json::to_string(&outcome.deleted)
            .unwrap_or_else(|_| "{}".to_string());
        self.with_conn({
            let error = outcome.error.clone();
            move |conn| {
                conn.execute(
                    "INSERT INTO prune_log (started_unix, finished_unix, ok, error, deleted_json) \
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![started, finished, ok as i64, error, deleted_json],
                )?;
                Ok(())
            }
        })
        .await?;
        match outcome.error {
            None => Ok(PruneReport {
                deleted: outcome.deleted,
                started_unix: started,
                finished_unix: finished,
            }),
            Some(e) => Err(AppError::Storage(format!("prune failed: {e}"))),
        }
    }

    /// P0 requirement 3 (AC5): `admin.usage`'s `db_bytes`,
    /// `db_page_free_bytes`, `rows_by_table`, and `last_prune`.
    /// `rows_by_table` counts every real (non-`sqlite_*`) table, not just
    /// the prunable ones, so an operator can see the whole database's
    /// shape, not only the part this PRD prunes.
    pub async fn usage_size_stats(&self) -> Result<UsageSizeStats, AppError> {
        self.with_conn(|conn| {
            let page_count: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
            let page_size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
            let freelist_count: i64 = conn.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
            let db_bytes = page_count * page_size;
            let db_page_free_bytes = freelist_count * page_size;

            let table_names: Vec<String> = {
                let mut stmt = conn.prepare(
                    "SELECT name FROM sqlite_master WHERE type = 'table' \
                     AND name NOT LIKE 'sqlite_%' ORDER BY name",
                )?;
                stmt.query_map([], |r| r.get(0))?
                    .collect::<Result<Vec<_>, _>>()?
            };
            let mut rows_by_table = Vec::with_capacity(table_names.len());
            for name in table_names {
                let count: i64 =
                    conn.query_row(&format!("SELECT COUNT(*) FROM \"{name}\""), [], |r| r.get(0))?;
                rows_by_table.push((name, count));
            }

            let last_prune = conn
                .query_row(
                    "SELECT started_unix, ok, deleted_json FROM prune_log ORDER BY id DESC LIMIT 1",
                    [],
                    |r| {
                        let at_unix: i64 = r.get(0)?;
                        let ok: i64 = r.get(1)?;
                        let deleted_json: String = r.get(2)?;
                        Ok((at_unix, ok != 0, deleted_json))
                    },
                )
                .optional()?
                .map(|(at_unix, ok, deleted_json)| {
                    let deleted: std::collections::BTreeMap<String, i64> =
                        serde_json::from_str(&deleted_json).unwrap_or_default();
                    LastPrune {
                        at_unix,
                        ok,
                        deleted,
                    }
                });

            Ok(UsageSizeStats {
                db_bytes,
                db_page_free_bytes,
                rows_by_table,
                last_prune,
            })
        })
        .await
    }

    /// `admin.mesh.threads(tenant?, channel?, limit)` (requirement 2 /
    /// AC2): thread summaries (participant addresses, `message_count`,
    /// `last_activity`, ordered most-recently-active first) plus channel
    /// summaries -- neither ever selects a `body` column (AC2: "no
    /// response field contains a body").
    pub async fn mesh_threads(
        &self,
        tenant_filter: Option<i64>,
        channel_filter: Option<String>,
        limit: i64,
    ) -> Result<(Vec<ThreadSummary>, Vec<ChannelThreadSummary>), AppError> {
        self.with_conn(move |conn| {
            let thread_ids: Vec<String> = match tenant_filter {
                Some(tid) => {
                    let mut stmt = conn.prepare(
                        "SELECT t.id FROM threads t JOIN thread_participants tp ON tp.thread_id = t.id \
                         WHERE tp.tenant_id = ?1 \
                         ORDER BY (SELECT MAX(m.created_unix_ms) FROM messages m WHERE m.thread_id = t.id) DESC \
                         LIMIT ?2",
                    )?;
                    stmt.query_map(params![tid, limit], |r| r.get(0))?
                        .collect::<rusqlite::Result<Vec<_>>>()?
                }
                None => {
                    let mut stmt = conn.prepare(
                        "SELECT t.id FROM threads t \
                         ORDER BY (SELECT MAX(m.created_unix_ms) FROM messages m WHERE m.thread_id = t.id) DESC \
                         LIMIT ?1",
                    )?;
                    stmt.query_map(params![limit], |r| r.get(0))?
                        .collect::<rusqlite::Result<Vec<_>>>()?
                }
            };
            let mut threads = Vec::with_capacity(thread_ids.len());
            for thread_id in thread_ids {
                let message_count: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM messages WHERE thread_id = ?1",
                    params![thread_id],
                    |r| r.get(0),
                )?;
                let last_activity: Option<String> = conn.query_row(
                    "SELECT MAX(created_at) FROM messages WHERE thread_id = ?1",
                    params![thread_id],
                    |r| r.get(0),
                )?;
                let mut pstmt = conn.prepare(
                    "SELECT t2.namespace FROM thread_participants tp JOIN tenants t2 ON t2.id = tp.tenant_id \
                     WHERE tp.thread_id = ?1 ORDER BY t2.namespace",
                )?;
                let participants: Vec<String> = pstmt
                    .query_map(params![thread_id], |r| r.get(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                threads.push(ThreadSummary {
                    thread_id,
                    participants,
                    message_count,
                    last_activity,
                });
            }

            let channel_rows: Vec<(String, String)> = match &channel_filter {
                Some(name) => {
                    let mut stmt = conn.prepare("SELECT id, name FROM channels WHERE name = ?1")?;
                    stmt.query_map(params![name], |r| Ok((r.get(0)?, r.get(1)?)))?
                        .collect::<rusqlite::Result<Vec<_>>>()?
                }
                None => {
                    let mut stmt = conn.prepare(
                        "SELECT id, name FROM channels ORDER BY created_unix_ms DESC LIMIT ?1",
                    )?;
                    stmt.query_map(params![limit], |r| Ok((r.get(0)?, r.get(1)?)))?
                        .collect::<rusqlite::Result<Vec<_>>>()?
                }
            };
            let mut channels = Vec::with_capacity(channel_rows.len());
            for (channel_id, name) in channel_rows {
                let post_count: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM channel_posts WHERE channel_id = ?1",
                    params![channel_id],
                    |r| r.get(0),
                )?;
                let last_activity: Option<String> = conn.query_row(
                    "SELECT MAX(created_at) FROM channel_posts WHERE channel_id = ?1",
                    params![channel_id],
                    |r| r.get(0),
                )?;
                channels.push(ChannelThreadSummary {
                    channel_id,
                    name,
                    post_count,
                    last_activity,
                });
            }
            Ok((threads, channels))
        })
        .await
    }

    /// `admin.mesh.thread(thread_id)` reading a thread (requirement 3):
    /// unlike [`Self::msg_thread`], no participant check -- the admin key
    /// may read any thread's bodies (this PRD's whole reason to exist: "the
    /// `admin.mesh.thread` body view exists for abuse review and is
    /// audited", not participant-gated like a tenant's own
    /// `host.msg.thread`). `None` if `thread_id` doesn't exist at all.
    pub async fn admin_thread_messages(
        &self,
        thread_id: String,
        after_seq: Option<i64>,
        limit: i64,
    ) -> Result<Option<Vec<MessageRow>>, AppError> {
        self.with_conn(move |conn| {
            let exists: bool = conn
                .prepare("SELECT 1 FROM threads WHERE id = ?1")?
                .exists(params![thread_id])?;
            if !exists {
                return Ok(None);
            }
            let mut sql = String::from(
                "SELECT m.id, m.thread_id, m.seq, m.from_address, m.body, m.data_json, m.in_reply_to, \
                        m.synthetic, m.source_class, m.created_at, m.created_unix_ms, NULL, m.urgent \
                 FROM messages m WHERE m.thread_id = ?1",
            );
            let mut sql_params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(thread_id.clone())];
            if let Some(seq) = after_seq {
                sql.push_str(&format!(" AND m.seq > ?{}", sql_params.len() + 1));
                sql_params.push(Box::new(seq));
            }
            sql.push_str(&format!(" ORDER BY m.seq LIMIT ?{}", sql_params.len() + 1));
            sql_params.push(Box::new(limit));
            let mut stmt = conn.prepare(&sql)?;
            let param_refs: Vec<&dyn rusqlite::ToSql> = sql_params.iter().map(|b| b.as_ref()).collect();
            let rows = stmt.query_map(param_refs.as_slice(), message_row_from_row)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(Some(out))
        })
        .await
    }

    /// `admin.mesh.thread(channel_id)` reading a channel's posts -- the
    /// channel-shaped counterpart to [`Self::admin_thread_messages`] above,
    /// consulted when `thread_or_channel_id` doesn't name a thread.
    /// `channel_ref` may be either a channel's `id` or its `name`. `None`
    /// if it names neither.
    pub async fn admin_channel_posts(
        &self,
        channel_ref: String,
        after_seq: Option<i64>,
        limit: i64,
    ) -> Result<Option<(String, Vec<ChannelPostRow>)>, AppError> {
        self.with_conn(move |conn| {
            let channel_id: Option<String> = conn
                .query_row(
                    "SELECT id FROM channels WHERE id = ?1 OR name = ?1",
                    params![channel_ref],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(channel_id) = channel_id else {
                return Ok(None);
            };
            let mut sql = String::from(
                "SELECT id, channel_id, seq, from_address, body, data_json, synthetic, \
                        source_class, created_at, created_unix_ms \
                 FROM channel_posts WHERE channel_id = ?1",
            );
            let mut sql_params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(channel_id.clone())];
            if let Some(seq) = after_seq {
                sql.push_str(&format!(" AND seq > ?{}", sql_params.len() + 1));
                sql_params.push(Box::new(seq));
            }
            sql.push_str(&format!(" ORDER BY seq LIMIT ?{}", sql_params.len() + 1));
            sql_params.push(Box::new(limit));
            let mut stmt = conn.prepare(&sql)?;
            let param_refs: Vec<&dyn rusqlite::ToSql> = sql_params.iter().map(|b| b.as_ref()).collect();
            let rows = stmt.query_map(param_refs.as_slice(), channel_post_row_from_row)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(Some((channel_id, out)))
        })
        .await
    }

    /// `host.channel.open(name)`: idempotent create-or-get by name.
    pub async fn channel_open(&self, tenant_id: i64, name: String) -> Result<ChannelRow, AppError> {
        let now = now_rfc3339();
        let now_ms = crate::state::now_unix_ms();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO channels (id, name, created_by, created_at, created_unix_ms) \
                 VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(name) DO NOTHING",
                params![crate::state::new_ulid(), name, tenant_id, now, now_ms],
            )?;
            let (id, created_at): (String, String) = conn.query_row(
                "SELECT id, created_at FROM channels WHERE name = ?1",
                params![name],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            Ok(ChannelRow { id, name, created_at })
        })
        .await
    }

    /// P1 requirement 6 (AC9): `/healthz`'s own `last_prune_ok` -- a
    /// lighter read than [`Self::usage_size_stats`] (skips the
    /// `rows_by_table` full-table-scan pass) for a check-every-request
    /// endpoint. `true` (nothing has failed yet) when no prune has ever
    /// run.
    pub async fn last_prune_ok(&self) -> Result<bool, AppError> {
        self.with_conn(|conn| {
            let ok: Option<i64> = conn
                .query_row("SELECT ok FROM prune_log ORDER BY id DESC LIMIT 1", [], |r| {
                    r.get(0)
                })
                .optional()?;
            Ok(ok.map(|v| v != 0).unwrap_or(true))
        })
        .await
    }

    /// `host.channel.post(channel, body, data?)`: `channel` may be either
    /// the name passed to `host.channel.open` or the `channel_id` it
    /// returned. Auto-advances the poster's own `channel_cursors` row to
    /// the new `seq` (AC7's "stored a cursor").
    pub async fn channel_post(
        &self,
        sender: Tenant,
        channel_ref: String,
        body: String,
        data_json: Option<String>,
    ) -> Result<ChannelPostOutcome, AppError> {
        let now = now_rfc3339();
        let now_ms = crate::state::now_unix_ms();
        self.with_conn(move |conn| {
            conn.execute("BEGIN IMMEDIATE", []).map_err(AppError::from)?;
            let outcome: Result<ChannelPostOutcome, AppError> = (|| {
                let channel_id: Option<String> = conn
                    .query_row(
                        "SELECT id FROM channels WHERE id = ?1 OR name = ?1",
                        params![channel_ref],
                        |r| r.get(0),
                    )
                    .optional()?;
                let Some(channel_id) = channel_id else {
                    return Err(AppError::channel_not_found());
                };
                let seq: i64 = conn.query_row(
                    "SELECT COALESCE(MAX(seq), 0) + 1 FROM channel_posts WHERE channel_id = ?1",
                    params![channel_id],
                    |r| r.get(0),
                )?;
                let post_id = crate::state::new_ulid();
                conn.execute(
                    "INSERT INTO channel_posts \
                        (id, channel_id, seq, from_tenant_id, from_address, body, data_json, \
                         synthetic, source_class, created_at, created_unix_ms) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    params![
                        post_id,
                        channel_id,
                        seq,
                        sender.id,
                        sender.namespace,
                        body,
                        data_json,
                        sender.synthetic,
                        sender.source_class.as_deref().unwrap_or("external"),
                        now,
                        now_ms
                    ],
                )?;
                conn.execute(
                    "INSERT INTO channel_cursors (channel_id, tenant_id, seq, updated_at) \
                     VALUES (?1, ?2, ?3, ?4) \
                     ON CONFLICT(channel_id, tenant_id) DO UPDATE SET seq = excluded.seq, updated_at = excluded.updated_at",
                    params![channel_id, sender.id, seq, now],
                )?;
                Ok(ChannelPostOutcome {
                    id: post_id,
                    channel_id: channel_id.clone(),
                    seq,
                    created_at: now.clone(),
                })
            })();
            match &outcome {
                Ok(_) => conn.execute("COMMIT", []).map(|_| ()).map_err(AppError::from)?,
                Err(_) => {
                    let _ = conn.execute("ROLLBACK", []);
                }
            }
            outcome
        })
        .await
    }

    // ---- group channels (PRD-mcphost-agent-channels) ----------------------

    /// `host.channel.open(group)` (requirement 2 / AC1): idempotent --
    /// re-opening an already-open group's channel returns the existing
    /// row rather than erroring or creating a second one (`idx_channels_
    /// group_id`'s partial unique index is what actually enforces "one
    /// channel per group" against a race; this idempotent-select-first
    /// path is the common case). `owner_tenant_id`/`group_name` name a
    /// group the CALLER owns (same "group is scoped by its owner" contract
    /// [`Self::create_group`]/[`Self::group_add_member`] already use) --
    /// [`AppError::GroupNotFound`] when no such group exists, same code
    /// `host.group.add`/`host.group.remove` already return for the same
    /// mistake.
    pub async fn channel_group_open(
        &self,
        owner_tenant_id: i64,
        group_name: String,
        channels_max: i64,
    ) -> Result<ChannelRow, AppError> {
        let now = now_rfc3339();
        let now_ms = crate::state::now_unix_ms();
        self.with_conn(move |conn| {
            let Some(group_id) = Self::find_group_id_sync(conn, owner_tenant_id, &group_name)?
            else {
                return Err(AppError::GroupNotFound(group_name));
            };
            let existing: Option<(String, String)> = conn
                .query_row(
                    "SELECT id, created_at FROM channels WHERE group_id = ?1",
                    params![group_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if let Some((id, created_at)) = existing {
                return Ok(ChannelRow { id, name: group_name, created_at });
            }
            let used: i64 = conn.query_row(
                "SELECT COUNT(*) FROM channels c JOIN groups g ON g.id = c.group_id \
                 WHERE g.owner_tenant_id = ?1",
                params![owner_tenant_id],
                |r| r.get(0),
            )?;
            if used >= channels_max {
                return Err(AppError::channel_quota_exceeded("channels_max", channels_max));
            }
            let id = crate::state::new_ulid();
            // Never exposed as `name` in any tool response (the group's own
            // name is what `channel_group_open`'s caller gets back) -- just
            // a value satisfying `channels.name`'s pre-existing `NOT NULL
            // UNIQUE` from migration 0027, distinguishable from any legacy
            // caller-chosen name by construction.
            let synthetic_name = format!("__group_channel__{group_id}");
            conn.execute(
                "INSERT INTO channels (id, name, group_id, created_by, created_at, created_unix_ms) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![id, synthetic_name, group_id, owner_tenant_id, now, now_ms],
            )?;
            Ok(ChannelRow { id, name: group_name, created_at: now.clone() })
        })
        .await
    }

    /// The group a channel id resolves to, if it's a group channel at all
    /// (a legacy migration-0027 name-only channel, or an id naming nothing,
    /// both answer `None` here -- [`Self::channel_group_lookup`]'s callers
    /// treat that the same as "caller isn't a member", so a non-member and
    /// a nonexistent id read byte-identically, AC2).
    pub async fn channel_group_lookup(
        &self,
        channel_id: String,
    ) -> Result<Option<GroupChannelCtx>, AppError> {
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT g.owner_tenant_id, g.name, c.closed_at, c.frozen_at \
                 FROM channels c JOIN groups g ON g.id = c.group_id WHERE c.id = ?1",
                params![channel_id],
                |r| {
                    Ok(GroupChannelCtx {
                        owner_tenant_id: r.get(0)?,
                        group_name: r.get(1)?,
                        closed_at: r.get(2)?,
                        frozen_at: r.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(AppError::from)
        })
        .await
    }

    /// AC1/AC2/AC5: is `tenant_id` allowed to read/post on a group channel
    /// whose group is owned by `owner_tenant_id` named `group_name` -- the
    /// owner always is (same as `host.group.add`/`remove`'s own owner-only
    /// contract), otherwise exactly [`Self::is_group_member`]'s own
    /// resolution-time check `tools/list`'s cross-tenant path already uses
    /// (technical considerations: "so channel and tool visibility can
    /// never disagree").
    pub async fn channel_group_is_authorized(
        &self,
        ctx: &GroupChannelCtx,
        tenant_id: i64,
    ) -> Result<bool, AppError> {
        if tenant_id == ctx.owner_tenant_id {
            return Ok(true);
        }
        self.is_group_member(ctx.owner_tenant_id, ctx.group_name.clone(), tenant_id).await
    }

    /// AC9/AC10: owner-only `closed_at`/`frozen_at` toggle -- `false` when
    /// `channel_id` doesn't exist, isn't a group channel, or isn't owned by
    /// `owner_tenant_id` (the caller's own `host.channel.close`/`freeze`/
    /// `unfreeze` turns that into the same [`AppError::channel_not_found`]
    /// every other "doesn't exist or isn't yours" case in this module uses).
    async fn channel_group_set_flag(
        &self,
        owner_tenant_id: i64,
        channel_id: String,
        column: &'static str,
        value: Option<String>,
    ) -> Result<bool, AppError> {
        self.with_conn(move |conn| {
            let sql = format!(
                "UPDATE channels SET {column} = ?1 WHERE id = ?2 AND group_id IN \
                 (SELECT id FROM groups WHERE owner_tenant_id = ?3)"
            );
            let affected = conn.execute(&sql, params![value, channel_id, owner_tenant_id])?;
            Ok(affected > 0)
        })
        .await
    }

    /// `host.channel.close` (AC9).
    pub async fn channel_group_close(
        &self,
        owner_tenant_id: i64,
        channel_id: String,
    ) -> Result<bool, AppError> {
        self.channel_group_set_flag(owner_tenant_id, channel_id, "closed_at", Some(now_rfc3339()))
            .await
    }

    /// `host.channel.freeze`/`unfreeze` (P1 requirement 10 / AC10).
    pub async fn channel_group_set_frozen(
        &self,
        owner_tenant_id: i64,
        channel_id: String,
        frozen: bool,
    ) -> Result<bool, AppError> {
        let value = if frozen { Some(now_rfc3339()) } else { None };
        self.channel_group_set_flag(owner_tenant_id, channel_id, "frozen_at", value)
            .await
    }

    /// AC1/AC3/AC4/AC5/AC8: every post after `after_seq`, in `seq` order --
    /// deleted (retention-purged) rows simply aren't there any more, so a
    /// cursor left below the retention horizon (AC8) starts at the first
    /// still-existing row with no special-casing needed here.
    pub async fn channel_posts_after(
        &self,
        channel_id: String,
        after_seq: i64,
        limit: i64,
    ) -> Result<Vec<ChannelPostRow>, AppError> {
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, channel_id, seq, from_address, body, data_json, synthetic, \
                        source_class, created_at, created_unix_ms \
                 FROM channel_posts WHERE channel_id = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3",
            )?;
            stmt.query_map(params![channel_id, after_seq, limit], channel_post_row_from_row)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(AppError::from)
        })
        .await
    }

    /// AC3/AC4: this tenant's stored read cursor for a group channel, if
    /// it has ever called `host.channel.read(ack: true)` on it.
    pub async fn channel_cursor_seq(
        &self,
        channel_id: String,
        tenant_id: i64,
    ) -> Result<Option<i64>, AppError> {
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT seq FROM channel_cursors WHERE channel_id = ?1 AND tenant_id = ?2",
                params![channel_id, tenant_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(AppError::from)
        })
        .await
    }

    /// AC3/AC4: `host.channel.read(ack: true)` stores `next_cursor` as this
    /// tenant's new `last_seq` for the channel.
    pub async fn channel_cursor_ack(
        &self,
        channel_id: String,
        tenant_id: i64,
        seq: i64,
    ) -> Result<(), AppError> {
        let now = now_rfc3339();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO channel_cursors (channel_id, tenant_id, seq, updated_at) \
                 VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT(channel_id, tenant_id) DO UPDATE SET seq = excluded.seq, \
                    updated_at = excluded.updated_at",
                params![channel_id, tenant_id, seq, now],
            )?;
            Ok(())
        })
        .await
    }

    /// AC1/AC3/AC4: `host.channel.post` on a group channel -- deliberately
    /// NOT [`Self::channel_post`]'s own behavior of auto-advancing the
    /// sender's read cursor to the new post: AC4 requires a poster's own
    /// posts to still come back on its own next `host.channel.read` (a
    /// poster is a reader too), so this table's cursor is touched only by
    /// an explicit `ack: true` read, never by posting.
    pub async fn channel_group_post_insert(
        &self,
        sender: Tenant,
        channel_id: String,
        body: String,
        data_json: Option<String>,
    ) -> Result<ChannelPostOutcome, AppError> {
        let now = now_rfc3339();
        let now_ms = crate::state::now_unix_ms();
        self.with_conn(move |conn| {
            conn.execute("BEGIN IMMEDIATE", []).map_err(AppError::from)?;
            let outcome: Result<ChannelPostOutcome, AppError> = (|| {
                let seq: i64 = conn.query_row(
                    "SELECT COALESCE(MAX(seq), 0) + 1 FROM channel_posts WHERE channel_id = ?1",
                    params![channel_id],
                    |r| r.get(0),
                )?;
                let post_id = crate::state::new_ulid();
                conn.execute(
                    "INSERT INTO channel_posts \
                        (id, channel_id, seq, from_tenant_id, from_address, body, data_json, \
                         synthetic, source_class, created_at, created_unix_ms) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    params![
                        post_id,
                        channel_id,
                        seq,
                        sender.id,
                        sender.namespace,
                        body,
                        data_json,
                        sender.synthetic,
                        sender.source_class.as_deref().unwrap_or("external"),
                        now,
                        now_ms
                    ],
                )?;
                Ok(ChannelPostOutcome { id: post_id, channel_id: channel_id.clone(), seq, created_at: now.clone() })
            })();
            match &outcome {
                Ok(_) => conn.execute("COMMIT", []).map(|_| ()).map_err(AppError::from)?,
                Err(_) => {
                    let _ = conn.execute("ROLLBACK", []);
                }
            }
            outcome
        })
        .await
    }

    /// AC7's quota check: how many `host.channel.post` calls `tenant_id`
    /// has made to any group channel in the sliding hour since `since_ms`
    /// -- scoped to `group_id IS NOT NULL` so migration 0027's legacy,
    /// ungated channel slice never counts against this PRD's own quota.
    pub async fn count_channel_posts_since(
        &self,
        tenant_id: i64,
        since_ms: i64,
    ) -> Result<i64, AppError> {
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM channel_posts p JOIN channels c ON c.id = p.channel_id \
                 WHERE c.group_id IS NOT NULL AND p.from_tenant_id = ?1 AND p.created_unix_ms >= ?2",
                params![tenant_id, since_ms],
                |r| r.get(0),
            )
            .map_err(AppError::from)
        })
        .await
    }

    /// AC6: every current member id of `owner_tenant_id`'s group
    /// `group_name` -- `messaging::fire_message_triggers`'s own delivered-
    /// tenant-ids loop, applied to a group's membership instead of a
    /// message's recipient list.
    pub async fn group_member_ids(
        &self,
        owner_tenant_id: i64,
        group_name: String,
    ) -> Result<Vec<i64>, AppError> {
        self.with_conn(move |conn| {
            let Some(group_id) = Self::find_group_id_sync(conn, owner_tenant_id, &group_name)?
            else {
                return Ok(Vec::new());
            };
            let mut stmt =
                conn.prepare("SELECT member_tenant_id FROM group_members WHERE group_id = ?1")?;
            stmt.query_map(params![group_id], |r| r.get(0))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(AppError::from)
        })
        .await
    }

    /// AC8's housekeeping tick: every group channel, alongside its owner's
    /// plan name (so the caller can look up that plan's own
    /// `channel_retention_days` -- retention is per the OWNER's plan, not
    /// the poster's, same "the channel's own owner sets the terms"
    /// convention `channels_max`/`channel_posts_per_hour` already lean on).
    pub async fn list_group_channels_with_owner_plan(
        &self,
    ) -> Result<Vec<(String, String)>, AppError> {
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT c.id, t.plan FROM channels c \
                 JOIN groups g ON g.id = c.group_id \
                 JOIN tenants t ON t.id = g.owner_tenant_id \
                 WHERE c.group_id IS NOT NULL",
            )?;
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(AppError::from)
        })
        .await
    }

    /// AC8: deletes a group channel's posts older than `cutoff_unix_ms` --
    /// the retention half of the housekeeping tick; a stale cursor below
    /// the new horizon needs no separate clamp (see
    /// [`Self::channel_posts_after`]'s own doc comment).
    pub async fn delete_channel_posts_older_than(
        &self,
        channel_id: String,
        cutoff_unix_ms: i64,
    ) -> Result<i64, AppError> {
        self.with_conn(move |conn| {
            let affected = conn.execute(
                "DELETE FROM channel_posts WHERE channel_id = ?1 AND created_unix_ms < ?2",
                params![channel_id, cutoff_unix_ms],
            )?;
            Ok(affected as i64)
        })
        .await
    }

    /// Test-only: insert one `calls` row at an arbitrary age (AC1/AC3/AC4
    /// fixtures need rows older than any real call in a fresh test
    /// server), bypassing `host.tool_call`'s real dispatch path so a test
    /// can seed thousands of rows in milliseconds. Mirrors the columns
    /// `Db::record_call` writes, with every metering/outcome/origin column
    /// defaulted since the retention prune reads only `started_unix`.
    pub async fn insert_calls_row_for_test(
        &self,
        tenant_id: i64,
        started_unix: i64,
    ) -> Result<(), AppError> {
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO calls (tenant_id, tool_name, started_at, started_unix, \
                 duration_ms, ok, error_class) VALUES (?1, 'test_tool', ?2, ?3, 0, 1, NULL)",
                params![tenant_id, crate::state::rfc3339_from_unix(started_unix), started_unix],
            )?;
            Ok(())
        })
        .await
    }

    fn mesh_count_real_synth(
        conn: &Connection,
        table: &str,
        since_ms: i64,
        tenant_id: Option<i64>,
        extra_where: Option<&str>,
    ) -> Result<(i64, i64), AppError> {
        let mut sql = format!(
            "SELECT SUM(CASE WHEN synthetic IS NULL THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN synthetic IS NOT NULL THEN 1 ELSE 0 END) \
             FROM {table} WHERE created_unix_ms >= ?1"
        );
        if let Some(id) = tenant_id {
            sql.push_str(&format!(" AND from_tenant_id = {id}"));
        }
        if let Some(extra) = extra_where {
            sql.push_str(&format!(" AND {extra}"));
        }
        let (real, synth): (Option<i64>, Option<i64>) =
            conn.query_row(&sql, params![since_ms], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok((real.unwrap_or(0), synth.unwrap_or(0)))
    }

    fn mesh_count_contact_requests_real_synth(
        conn: &Connection,
        since_ms: i64,
        tenant_id: Option<i64>,
    ) -> Result<(i64, i64), AppError> {
        let mut sql = String::from(
            "SELECT SUM(CASE WHEN t.synthetic IS NULL THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN t.synthetic IS NOT NULL THEN 1 ELSE 0 END) \
             FROM contact_requests cr JOIN tenants t ON t.id = cr.from_tenant_id \
             WHERE cr.created_unix_ms >= ?1",
        );
        if let Some(id) = tenant_id {
            sql.push_str(&format!(" AND cr.from_tenant_id = {id}"));
        }
        let (real, synth): (Option<i64>, Option<i64>) =
            conn.query_row(&sql, params![since_ms], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok((real.unwrap_or(0), synth.unwrap_or(0)))
    }

    /// Requirement 8 / AC9: every refusal code stored in `messages.refused_json`
    /// within the window, tallied both overall and per sender (`from_address`)
    /// -- "so a tenant hammering `contact_refused` is visible before anyone
    /// complains."
    fn mesh_tally_refusals(
        conn: &Connection,
        since_ms: i64,
        tenant_id: Option<i64>,
    ) -> Result<(RefusalCounts, RefusalCountsBySender), AppError> {
        use std::collections::{BTreeMap, HashMap};
        let mut sql = String::from(
            "SELECT from_address, refused_json FROM messages \
             WHERE created_unix_ms >= ?1 AND refused_json IS NOT NULL",
        );
        if let Some(id) = tenant_id {
            sql.push_str(&format!(" AND from_tenant_id = {id}"));
        }
        let mut stmt = conn.prepare(&sql)?;
        let rows: Vec<(String, String)> = stmt
            .query_map(params![since_ms], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut overall: BTreeMap<String, i64> = BTreeMap::new();
        let mut per_sender: HashMap<String, BTreeMap<String, i64>> = HashMap::new();
        for (from_address, refused_json) in rows {
            let Ok(pairs) = serde_json::from_str::<Vec<(String, String)>>(&refused_json) else {
                continue;
            };
            for (_addr, code) in pairs {
                *overall.entry(code.clone()).or_insert(0) += 1;
                *per_sender.entry(from_address.clone()).or_default().entry(code).or_insert(0) += 1;
            }
        }
        Ok((overall, per_sender))
    }

    /// `admin.mesh.stats(window, tenant?)` (requirement 1 / AC1; requirement
    /// 8 / AC9): every count split `{real, synthetic}` off the sender's
    /// stored label at send time (`messages.synthetic`/`channel_posts.synthetic`,
    /// or a live join to `tenants.synthetic` for `contact_requests`, which
    /// stores no label of its own); the per-tenant list is the top 20
    /// senders by message volume within the window, descending.
    pub async fn mesh_stats(
        &self,
        since_ms: i64,
        tenant_filter: Option<String>,
    ) -> Result<MeshStats, AppError> {
        self.with_conn(move |conn| {
            let tenant_id: Option<i64> = match &tenant_filter {
                Some(ns) => Some(
                    conn.query_row(
                        "SELECT id FROM tenants WHERE namespace = ?1",
                        params![ns],
                        |r| r.get(0),
                    )
                    .optional()?
                    .ok_or_else(|| AppError::TenantNotFound(ns.clone()))?,
                ),
                None => None,
            };

            let (messages_real, messages_synth) =
                Self::mesh_count_real_synth(conn, "messages", since_ms, tenant_id, None)?;
            let (posts_real, posts_synth) =
                Self::mesh_count_real_synth(conn, "channel_posts", since_ms, tenant_id, None)?;
            let (urgent_real, urgent_synth) =
                Self::mesh_count_real_synth(conn, "messages", since_ms, tenant_id, Some("urgent = 1"))?;
            let (cr_real, cr_synth) =
                Self::mesh_count_contact_requests_real_synth(conn, since_ms, tenant_id)?;
            let (overall_refusals, per_sender_refusals) =
                Self::mesh_tally_refusals(conn, since_ms, tenant_id)?;

            let active_pairs: i64 = conn.query_row(
                "SELECT COUNT(*) FROM (SELECT m.thread_id FROM messages m WHERE m.created_unix_ms >= ?1 \
                 GROUP BY m.thread_id \
                 HAVING (SELECT COUNT(*) FROM thread_participants tp WHERE tp.thread_id = m.thread_id) = 2)",
                params![since_ms],
                |r| r.get(0),
            )?;
            let active_channels: i64 = conn.query_row(
                "SELECT COUNT(DISTINCT channel_id) FROM channel_posts WHERE created_unix_ms >= ?1",
                params![since_ms],
                |r| r.get(0),
            )?;
            let wake_runs: i64 = conn.query_row(
                "SELECT COUNT(*) FROM runs WHERE trigger = 'message' AND started_unix >= ?1",
                params![since_ms / 1000],
                |r| r.get(0),
            )?;

            let mut top_sql = String::from(
                "SELECT m.from_address, COUNT(*) as cnt, \
                        (SELECT t.synthetic FROM tenants t WHERE t.id = m.from_tenant_id) as synthetic \
                 FROM messages m WHERE m.created_unix_ms >= ?1",
            );
            if let Some(id) = tenant_id {
                top_sql.push_str(&format!(" AND m.from_tenant_id = {id}"));
            }
            top_sql.push_str(" GROUP BY m.from_address ORDER BY cnt DESC LIMIT 20");
            let mut stmt = conn.prepare(&top_sql)?;
            let top_senders: Vec<(String, i64, Option<String>)> = stmt
                .query_map(params![since_ms], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            let mut tenants = Vec::with_capacity(top_senders.len());
            for (from_address, cnt, synthetic) in top_senders {
                let channel_posts_cnt: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM channel_posts WHERE from_address = ?1 AND created_unix_ms >= ?2",
                    params![from_address, since_ms],
                    |r| r.get(0),
                )?;
                let contact_requests_cnt: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM contact_requests cr JOIN tenants t ON t.id = cr.from_tenant_id \
                     WHERE t.namespace = ?1 AND cr.created_unix_ms >= ?2",
                    params![from_address, since_ms],
                    |r| r.get(0),
                )?;
                let urgent_cnt: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM messages WHERE from_address = ?1 AND urgent = 1 AND created_unix_ms >= ?2",
                    params![from_address, since_ms],
                    |r| r.get(0),
                )?;
                let refusals_by_code = per_sender_refusals.get(&from_address).cloned().unwrap_or_default();
                tenants.push(MeshTenantStats {
                    tenant: from_address,
                    synthetic,
                    messages: cnt,
                    channel_posts: channel_posts_cnt,
                    contact_requests: contact_requests_cnt,
                    urgent: urgent_cnt,
                    refusals_by_code,
                });
            }

            Ok(MeshStats {
                messages: MeshCounts { real: messages_real, synthetic: messages_synth },
                channel_posts: MeshCounts { real: posts_real, synthetic: posts_synth },
                contact_requests: MeshCounts { real: cr_real, synthetic: cr_synth },
                urgent: MeshCounts { real: urgent_real, synthetic: urgent_synth },
                refusals_by_code: overall_refusals,
                wake_runs,
                active_pairs,
                active_channels,
                tenants,
            })
        })
        .await
    }

    /// Test-only: the same fixture as
    /// [`Self::insert_calls_row_for_test`], `count` rows at once inside one
    /// transaction (AC4's 20,000-row batch-delete fixture) -- committing
    /// one row at a time for that many rows would make the test itself the
    /// slow part.
    pub async fn insert_calls_rows_bulk_for_test(
        &self,
        tenant_id: i64,
        count: i64,
        started_unix: i64,
    ) -> Result<(), AppError> {
        let started_at = crate::state::rfc3339_from_unix(started_unix);
        self.with_conn(move |conn| {
            let tx = conn.unchecked_transaction()?;
            {
                let mut stmt = tx.prepare(
                    "INSERT INTO calls (tenant_id, tool_name, started_at, started_unix, \
                     duration_ms, ok, error_class) VALUES (?1, 'test_tool', ?2, ?3, 0, 1, NULL)",
                )?;
                for _ in 0..count {
                    stmt.execute(params![tenant_id, started_at, started_unix])?;
                }
            }
            tx.commit()?;
            Ok(())
        })
        .await
    }

    /// `admin.mesh.purge(older_than_days, dry_run)` (requirement 5 / AC6):
    /// counts (dry run) or deletes (real run) `messages`/`channel_posts`
    /// older than `cutoff_unix_ms`. Deleting a `messages` row cascades its
    /// `message_receipts` for free (migration 0021's `ON DELETE CASCADE`);
    /// afterward, every channel's `channel_cursors` rows are clamped up to
    /// that channel's first surviving `seq` so a cursor never references a
    /// purged post. Writes one `admin_events` row on the real run only.
    pub async fn mesh_purge(&self, cutoff_unix_ms: i64, dry_run: bool) -> Result<MeshPurgeCounts, AppError> {
        self.with_conn(move |conn| {
            let messages_removed: i64 = conn.query_row(
                "SELECT COUNT(*) FROM messages WHERE created_unix_ms < ?1",
                params![cutoff_unix_ms],
                |r| r.get(0),
            )?;
            let channel_posts_removed: i64 = conn.query_row(
                "SELECT COUNT(*) FROM channel_posts WHERE created_unix_ms < ?1",
                params![cutoff_unix_ms],
                |r| r.get(0),
            )?;
            if dry_run {
                return Ok(MeshPurgeCounts { messages_removed, channel_posts_removed });
            }
            conn.execute("BEGIN IMMEDIATE", []).map_err(AppError::from)?;
            let outcome: Result<(), AppError> = (|| {
                conn.execute("DELETE FROM messages WHERE created_unix_ms < ?1", params![cutoff_unix_ms])?;
                conn.execute(
                    "DELETE FROM channel_posts WHERE created_unix_ms < ?1",
                    params![cutoff_unix_ms],
                )?;
                let channel_ids: Vec<String> = {
                    let mut stmt = conn.prepare("SELECT id FROM channels")?;
                    stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<Vec<_>>>()?
                };
                for channel_id in channel_ids {
                    let min_seq: Option<i64> = conn.query_row(
                        "SELECT MIN(seq) FROM channel_posts WHERE channel_id = ?1",
                        params![channel_id],
                        |r| r.get(0),
                    )?;
                    if let Some(min_seq) = min_seq {
                        conn.execute(
                            "UPDATE channel_cursors SET seq = ?1 WHERE channel_id = ?2 AND seq < ?1",
                            params![min_seq, channel_id],
                        )?;
                    }
                }
                let detail = json!({
                    "messages_removed": messages_removed,
                    "channel_posts_removed": channel_posts_removed,
                    "cutoff_unix_ms": cutoff_unix_ms,
                })
                .to_string();
                conn.execute(
                    "INSERT INTO admin_events (ts, action, tenant, detail) VALUES (?1, ?2, NULL, ?3)",
                    params![now_rfc3339(), "mesh.purge", detail],
                )?;
                Ok(())
            })();
            match &outcome {
                Ok(()) => conn.execute("COMMIT", []).map(|_| ()).map_err(AppError::from)?,
                Err(_) => {
                    let _ = conn.execute("ROLLBACK", []);
                }
            }
            outcome?;
            Ok(MeshPurgeCounts { messages_removed, channel_posts_removed })
        })
        .await
    }

    /// Test-only: insert one `meter_events` row at an arbitrary age (AC2's
    /// "399-day-old metering rows survive a 400-day window" fixture).
    /// `meter_events.created_at` is the RFC 3339 text column the real
    /// prune compares against, so this writes that column directly rather
    /// than a `created_unix` this table doesn't have.
    pub async fn insert_meter_event_row_for_test(
        &self,
        tenant_id: i64,
        created_unix: i64,
    ) -> Result<(), AppError> {
        let created_at = crate::state::rfc3339_from_unix(created_unix);
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO meter_events (batch_id, tenant_id, first_call_id, last_call_id, \
                 count, mode, created_at) VALUES ('test-batch', ?1, 0, 0, 1, 'sent', ?2)",
                params![tenant_id, created_at],
            )?;
            Ok(())
        })
        .await
    }

    /// Test/ops-only: backdate a `messages` row's `created_unix_ms` -- AC1's
    /// "last hour" window and AC6's 40/10-day-old purge fixtures both need
    /// a way to simulate age without a real wait, same convention as
    /// [`Self::test_backdate_contact_request`].
    pub async fn test_backdate_message(&self, message_id: String, created_unix_ms: i64) -> Result<(), AppError> {
        self.with_conn(move |conn| {
            conn.execute(
                "UPDATE messages SET created_unix_ms = ?1 WHERE id = ?2",
                params![created_unix_ms, message_id],
            )?;
            Ok(())
        })
        .await
    }

    /// Test-only: `SELECT COUNT(*)` on an arbitrary table name (AC3's "a
    /// table not in the policy is unchanged" proof needs a row count for a
    /// table the retention engine never touches, e.g. `tenants`). Never
    /// called with anything but a fixed string literal from test code.
    pub async fn table_row_count_for_test(&self, table: String) -> Result<i64, AppError> {
        self.with_conn(move |conn| {
            let count: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM \"{table}\""), [], |r| {
                r.get(0)
            })?;
            Ok(count)
        })
        .await
    }

    /// Test/ops-only: same as [`Self::test_backdate_message`], for a
    /// `channel_posts` row (AC6's channel-cursor clamp).
    pub async fn test_backdate_channel_post(&self, post_id: String, created_unix_ms: i64) -> Result<(), AppError> {
        self.with_conn(move |conn| {
            conn.execute(
                "UPDATE channel_posts SET created_unix_ms = ?1 WHERE id = ?2",
                params![created_unix_ms, post_id],
            )?;
            Ok(())
        })
        .await
    }
}

/// [`Db::prune_once`]'s return shape: one cycle's per-physical-table
/// deleted counts and its wall-clock span.
#[derive(Debug)]
pub struct PruneReport {
    pub deleted: std::collections::BTreeMap<String, i64>,
    pub started_unix: i64,
    pub finished_unix: i64,
}

/// [`Db::usage_size_stats`]'s `last_prune` field.
pub struct LastPrune {
    pub at_unix: i64,
    pub ok: bool,
    pub deleted: std::collections::BTreeMap<String, i64>,
}

/// [`Db::usage_size_stats`]'s return shape.
pub struct UsageSizeStats {
    pub db_bytes: i64,
    pub db_page_free_bytes: i64,
    pub rows_by_table: Vec<(String, i64)>,
    pub last_prune: Option<LastPrune>,
}

/// [`Db::mesh_tally_refusals`]'s own return shape: refusal-code -> count.
type RefusalCounts = std::collections::BTreeMap<String, i64>;
/// [`Db::mesh_tally_refusals`]'s per-sender breakdown: sender address ->
/// [`RefusalCounts`].
type RefusalCountsBySender = std::collections::HashMap<String, RefusalCounts>;

/// One channel row, as `host.channel.open`/[`Db::channel_open`] returns it.
#[derive(Debug, Clone, Serialize)]
pub struct ChannelRow {
    pub id: String,
    pub name: String,
    pub created_at: String,
}

/// PRD-mcphost-agent-channels: what [`Db::channel_group_lookup`] resolves a
/// group channel's `id` to -- the group's own owner/name (membership and
/// owner-only checks both key off these, never a separate `channels.name`)
/// plus its two lifecycle flags.
pub struct GroupChannelCtx {
    pub owner_tenant_id: i64,
    pub group_name: String,
    pub closed_at: Option<String>,
    pub frozen_at: Option<String>,
}

/// `host.channel.post`'s own outcome -- deliberately not [`SendOutcome`]
/// (no `delivered_to`/`refused`: a channel post has no per-recipient
/// resolution, every participant just reads it later).
pub struct ChannelPostOutcome {
    pub id: String,
    pub channel_id: String,
    pub seq: i64,
    pub created_at: String,
}

/// One `admin.mesh.thread` row for a channel -- same shape as [`MessageRow`]
/// minus `in_reply_to`/`read_at` (a channel post has neither).
#[derive(Debug, Clone, Serialize)]
pub struct ChannelPostRow {
    pub id: String,
    pub channel_id: String,
    pub seq: i64,
    pub from_address: String,
    pub body: String,
    pub data: Option<Value>,
    pub synthetic: Option<String>,
    pub source_class: String,
    pub created_at: String,
    pub created_unix_ms: i64,
}

fn channel_post_row_from_row(r: &Row) -> rusqlite::Result<ChannelPostRow> {
    let data_text: Option<String> = r.get(5)?;
    Ok(ChannelPostRow {
        id: r.get(0)?,
        channel_id: r.get(1)?,
        seq: r.get(2)?,
        from_address: r.get(3)?,
        body: r.get(4)?,
        data: data_text.and_then(|s| serde_json::from_str(&s).ok()),
        synthetic: r.get(6)?,
        source_class: r.get(7)?,
        created_at: r.get(8)?,
        created_unix_ms: r.get(9)?,
    })
}

/// `admin.mesh.threads`' own per-thread summary row (requirement 2 / AC2):
/// participant addresses, `message_count`, `last_activity` -- deliberately
/// no `body` field anywhere in this shape (AC2: "no response field
/// contains a body").
#[derive(Debug, Clone, Serialize)]
pub struct ThreadSummary {
    pub thread_id: String,
    pub participants: Vec<String>,
    pub message_count: i64,
    pub last_activity: Option<String>,
}

/// `admin.mesh.threads`' channel counterpart -- `post_count`/`last_activity`
/// in place of a thread's `message_count`/`last_activity`; no membership
/// model exists for a channel yet (this PRD's own minimal slice, see
/// `channels.rs`'s module doc), so there is no `participants` field here.
#[derive(Debug, Clone, Serialize)]
pub struct ChannelThreadSummary {
    pub channel_id: String,
    pub name: String,
    pub post_count: i64,
    pub last_activity: Option<String>,
}

/// One side of a `{real, synthetic}` split every `admin.mesh.stats` count
/// uses (requirement 1).
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct MeshCounts {
    pub real: i64,
    pub synthetic: i64,
}

/// `admin.mesh.stats`' per-tenant breakdown -- the top 20 senders by
/// message volume within the window, descending (AC1).
#[derive(Debug, Clone, Serialize)]
pub struct MeshTenantStats {
    pub tenant: String,
    pub synthetic: Option<String>,
    pub messages: i64,
    pub channel_posts: i64,
    pub contact_requests: i64,
    pub urgent: i64,
    /// Requirement 8 / AC9: this sender's own refused-send codes, tallied
    /// from every one of its messages' stored `refused_json` within the
    /// window.
    pub refusals_by_code: std::collections::BTreeMap<String, i64>,
}

/// [`Db::mesh_stats`]'s whole return shape (requirement 1 / AC1, AC9).
#[derive(Debug, Clone, Serialize)]
pub struct MeshStats {
    pub messages: MeshCounts,
    pub channel_posts: MeshCounts,
    pub contact_requests: MeshCounts,
    pub urgent: MeshCounts,
    pub refusals_by_code: std::collections::BTreeMap<String, i64>,
    pub wake_runs: i64,
    pub active_pairs: i64,
    pub active_channels: i64,
    pub tenants: Vec<MeshTenantStats>,
}

/// [`Db::mesh_purge`]'s return shape -- the same counts for a dry run
/// (nothing removed) or a real one (already removed) (AC6).
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct MeshPurgeCounts {
    pub messages_removed: i64,
    pub channel_posts_removed: i64,
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
