//! SQLite storage. One dedicated blocking task services every query
//! (`spawn_blocking` around a mutex-guarded [`rusqlite::Connection`]) so a
//! slow query never stalls the async runtime, per the PRD's technical
//! considerations.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde_json::Value;

use crate::errors::AppError;

const MIGRATION_0001: &str = include_str!("../migrations/0001_init.sql");

#[derive(Debug, Clone, Serialize)]
pub struct Tenant {
    pub id: i64,
    pub namespace: String,
    pub display_name: String,
    pub key_hash: String,
    pub created_at: String,
    pub disabled: bool,
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
        conn.execute_batch(MIGRATION_0001).map_err(AppError::from)
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
                "SELECT id, namespace, display_name, key_hash, created_at, disabled \
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
                "SELECT id, namespace, display_name, key_hash, created_at, disabled \
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

    pub async fn list_tenants(&self) -> Result<Vec<Tenant>, AppError> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, namespace, display_name, key_hash, created_at, disabled \
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
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
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

    // ---- calls / metering ------------------------------------------------

    pub async fn record_call(
        &self,
        tenant_id: i64,
        tool_name: String,
        duration_ms: i64,
        ok: bool,
        error_class: Option<String>,
    ) -> Result<(), AppError> {
        let started_at = now_rfc3339();
        let started_unix = now_unix();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO calls (tenant_id, tool_name, started_at, started_unix, duration_ms, ok, error_class) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![tenant_id, tool_name, started_at, started_unix, duration_ms, ok as i64, error_class],
            )?;
            Ok(())
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
            let mut grouped: BTreeMap<(String, String), Vec<(i64, bool)>> = BTreeMap::new();
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
