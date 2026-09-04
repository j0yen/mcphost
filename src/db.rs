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

/// Shared by every query that selects a whole tenant row, so the column
/// list and [`tenant_from_row`] stay in lockstep with each other.
const TENANT_COLUMNS: &str = "id, namespace, display_name, key_hash, created_at, disabled, \
    last_tool_change_unix, namespace_verified, registry_namespace";

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
        Self::migrate_0005_cascade_delete(&conn)
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
    ) -> Result<Tenant, AppError> {
        let created_at = now_rfc3339();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO tenants (namespace, display_name, key_hash, created_at, disabled) \
                 VALUES (?1, ?2, ?3, ?4, 0)",
                params![namespace, display_name, key_hash, created_at],
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
            })
        })
        .await
    }

    pub async fn find_tenant_by_key_hash(
        &self,
        key_hash: String,
    ) -> Result<Option<Tenant>, AppError> {
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT id, namespace, display_name, key_hash, created_at, disabled, \
                        last_tool_change_unix, namespace_verified, registry_namespace \
                 FROM tenants WHERE key_hash = ?1",
                params![key_hash],
                |r| {
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
                    })
                },
            )
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
            conn.query_row(
                "SELECT id, namespace, display_name, key_hash, created_at, disabled, \
                        last_tool_change_unix, namespace_verified, registry_namespace \
                 FROM tenants WHERE namespace = ?1",
                params![namespace],
                |r| {
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
                    })
                },
            )
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
            let mut stmt = conn.prepare(
                "SELECT id, namespace, display_name, key_hash, created_at, disabled, \
                        last_tool_change_unix, namespace_verified, registry_namespace \
                 FROM tenants ORDER BY id",
            )?;
            let rows = stmt
                .query_map([], |r| {
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
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
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

    pub async fn record_signup_event(&self, source_ip: String) -> Result<(), AppError> {
        let ts = now_unix();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO signup_events (source_ip, created_unix) VALUES (?1, ?2)",
                params![source_ip, ts],
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
    ) -> Result<(), AppError> {
        let started_at = now_rfc3339();
        let started_unix = now_unix();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO calls (tenant_id, tool_name, started_at, started_unix, duration_ms, ok, error_class, cpu_ms, peak_rss_kb) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![tenant_id, tool_name, started_at, started_unix, duration_ms, ok as i64, error_class, cpu_ms, peak_rss_kb],
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
}
