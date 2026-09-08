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

/// Shared by every query that selects a whole tenant row, so the column
/// list and [`tenant_from_row`] stay in lockstep with each other.
const TENANT_COLUMNS: &str = "id, namespace, display_name, key_hash, created_at, disabled, \
    last_tool_change_unix, namespace_verified, registry_namespace, plan, plan_since, billing_ref, \
    stripe_customer_id, synthetic";

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
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolRow {
    pub id: i64,
    pub tenant_id: i64,
    pub name: String,
    pub kind: String,
    pub spec: Value,
    pub created_at: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct UsageStats {
    pub calls: i64,
    pub errors: i64,
    pub p50_ms: f64,
    pub p95_ms: f64,
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
        Self::migrate_0009_call_outcome(&conn)
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
        let created_at = now_rfc3339();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO tenants (namespace, display_name, key_hash, created_at, disabled, synthetic) \
                 VALUES (?1, ?2, ?3, ?4, 0, ?5)",
                params![namespace, display_name, key_hash, created_at, synthetic],
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
            })
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

    /// PRD-mcphost-synthetic-flag AC10 / P1 requirement 10: `(real, synthetic)`
    /// paying-tenant counts in one query, so `/healthz` can add
    /// `paying_tenants_real` only when the synthetic half is nonzero
    /// (guards against a synthetic tenant ever polluting the revenue count
    /// while keeping the field absent -- not present-and-equal -- on a host
    /// where it can never have differed from `paying_tenants`).
    pub async fn paying_tenant_synthetic_split(&self) -> Result<(i64, i64), AppError> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT \
                    SUM(CASE WHEN plan != 'free' AND synthetic IS NULL THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN plan != 'free' AND synthetic IS NOT NULL THEN 1 ELSE 0 END) \
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

    /// `synthetic` (P2 requirement 8) is the same validated-or-null label
    /// `create_tenant` stores on the tenant row, recorded here too so the
    /// ledger of signup attempts is independently auditable even for a
    /// tenant later deleted or retro-tagged differently.
    pub async fn record_signup_event(
        &self,
        source_ip: String,
        synthetic: Option<String>,
    ) -> Result<(), AppError> {
        let ts = now_unix();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO signup_events (source_ip, created_unix, synthetic) VALUES (?1, ?2, ?3)",
                params![source_ip, ts, synthetic],
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
            conn.query_row(
                "SELECT id, tenant_id, name, kind, spec, created_at FROM tools \
                 WHERE tenant_id = ?1 AND name = ?2",
                params![tenant_id, name],
                |r| {
                    let spec_text: String = r.get(4)?;
                    Ok(ToolRow {
                        id: r.get(0)?,
                        tenant_id: r.get(1)?,
                        name: r.get(2)?,
                        kind: r.get(3)?,
                        spec: serde_json::from_str(&spec_text).unwrap_or(Value::Null),
                        created_at: r.get(5)?,
                    })
                },
            )
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
            let mut stmt = conn.prepare(
                "SELECT id, tenant_id, name, kind, spec, created_at FROM tools \
                 WHERE tenant_id = ?1 ORDER BY name",
            )?;
            let rows = stmt
                .query_map(params![tenant_id], |r| {
                    let spec_text: String = r.get(4)?;
                    Ok(ToolRow {
                        id: r.get(0)?,
                        tenant_id: r.get(1)?,
                        name: r.get(2)?,
                        kind: r.get(3)?,
                        spec: serde_json::from_str(&spec_text).unwrap_or(Value::Null),
                        created_at: r.get(5)?,
                    })
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
    ) -> Result<(), AppError> {
        let started_at = now_rfc3339();
        let started_unix = now_unix();
        let outcome = outcome.to_string();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO calls (tenant_id, tool_name, started_at, started_unix, duration_ms, ok, error_class, cpu_ms, peak_rss_kb, outcome) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![tenant_id, tool_name, started_at, started_unix, duration_ms, ok as i64, error_class, cpu_ms, peak_rss_kb, outcome],
            )?;
            Ok(())
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
    pub async fn count_calls_since(
        &self,
        tenant_id: i64,
        since_unix: i64,
        ok_only: bool,
    ) -> Result<i64, AppError> {
        self.with_conn(move |conn| {
            let sql = if ok_only {
                "SELECT COUNT(*) FROM calls WHERE tenant_id = ?1 AND started_unix >= ?2 AND ok = 1"
            } else {
                "SELECT COUNT(*) FROM calls WHERE tenant_id = ?1 AND started_unix >= ?2"
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
                "SELECT duration_ms, ok FROM calls WHERE tenant_id = ?1 AND started_unix >= ?2",
            )?;
            let rows = stmt
                .query_map(params![tenant_id, since], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)? != 0))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            let mut durations: Vec<i64> = rows.iter().map(|(d, _)| *d).collect();
            durations.sort_unstable();
            let errors = rows.iter().filter(|(_, ok)| !ok).count() as i64;
            Ok(UsageStats {
                calls: rows.len() as i64,
                errors,
                p50_ms: percentile(&durations, 0.50),
                p95_ms: percentile(&durations, 0.95),
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
                "SELECT t.namespace, c.tool_name, c.duration_ms, c.ok \
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
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;

            use std::collections::BTreeMap;
            // Iteration 1 (rustbuild Stage 3, 2026-09-02): clippy's
            // type-complexity threshold was scaffolded tighter (200) than
            // default (250); factor the grouping type out per the lint's
            // own suggestion rather than raise the read-only threshold.
            type CallDurationsByTenantTool = BTreeMap<(String, String), Vec<(i64, bool)>>;
            let mut grouped: CallDurationsByTenantTool = BTreeMap::new();
            for (ns, tool, dur, ok) in rows {
                grouped.entry((ns, tool)).or_default().push((dur, ok));
            }
            let mut out = Vec::with_capacity(grouped.len());
            for ((namespace, tool_name), entries) in grouped {
                let mut durations: Vec<i64> = entries.iter().map(|(d, _)| *d).collect();
                durations.sort_unstable();
                let errors = entries.iter().filter(|(_, ok)| !ok).count() as i64;
                out.push(ToolUsage {
                    namespace,
                    tool_name,
                    stats: UsageStats {
                        calls: entries.len() as i64,
                        errors,
                        p50_ms: percentile(&durations, 0.50),
                        p95_ms: percentile(&durations, 0.95),
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
