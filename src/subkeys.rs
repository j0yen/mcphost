//! PRD-mcphost-key-scoped-to-tools: sub-keys -- a second hash-at-rest
//! credential per tenant, narrowed to a tool allowlist.
//!
//! A sub-key authenticates like the tenant key (header or `tenant_key`
//! argument) but resolves through `tenant_keys`, never touching
//! `tenants.key_hash`, so `host.key.rotate` leaves every sub-key alive.

use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};

use crate::auth::{generate_sub_key, hash_key};
use crate::db::{Db, Tenant};
use crate::errors::AppError;
use crate::state::{AppState, now_unix};

/// `ttl_s` bounds and default (R2).
pub const TTL_MIN_S: i64 = 60;
pub const TTL_MAX_S: i64 = 7_776_000;
pub const TTL_DEFAULT_S: i64 = 2_592_000;
/// `tools` length bounds (R2).
pub const TOOLS_MAX: usize = 64;
/// Longest accepted `label`.
pub const LABEL_MAX: usize = 64;

tokio::task_local! {
    /// The sub-key id the current `tools/call` authenticated with, filled by
    /// `McpHostHandler::call_tool_inner` after resolution and read by the
    /// `calls` insert (`Db::record_call_attributed_*`) so every row a
    /// sub-key's call writes carries `calls.key_id` without threading a new
    /// parameter through each call site.
    pub static CALL_KEY: std::cell::Cell<Option<i64>>;
}

/// The sub-key id of the call running on this task, if any.
pub fn current_key_id() -> Option<i64> {
    CALL_KEY.try_with(|c| c.get()).ok().flatten()
}

/// One `tenant_keys` row.
#[derive(Clone, Debug)]
pub struct SubKey {
    pub id: i64,
    pub tenant_id: i64,
    pub key_hash: String,
    pub label: String,
    pub tools: Vec<String>,
    pub created_at: i64,
    pub expires_at: i64,
    pub revoked_at: Option<i64>,
    pub last_used_at: Option<i64>,
}

const SUB_KEY_COLUMNS: &str =
    "id, tenant_id, key_hash, label, tools_json, created_at, expires_at, revoked_at, last_used_at";

fn sub_key_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<SubKey> {
    let tools_json: String = r.get(4)?;
    Ok(SubKey {
        id: r.get(0)?,
        tenant_id: r.get(1)?,
        key_hash: r.get(2)?,
        label: r.get(3)?,
        tools: serde_json::from_str(&tools_json).unwrap_or_default(),
        created_at: r.get(5)?,
        expires_at: r.get(6)?,
        revoked_at: r.get(7)?,
        last_used_at: r.get(8)?,
    })
}

/// The columns [`Db::insert_sub_key`] writes.
pub struct NewSubKey {
    pub key_hash: String,
    pub label: String,
    pub tools: Vec<String>,
    pub created_at: i64,
    pub expires_at: i64,
}

impl Db {
    /// Insert one sub-key; a label already used by this tenant is
    /// `label_taken`.
    pub async fn insert_sub_key(&self, tenant_id: i64, new: NewSubKey) -> Result<i64, AppError> {
        let NewSubKey { key_hash, label, tools, created_at, expires_at } = new;
        self.with_conn(move |conn| {
            let taken: bool = conn
                .prepare("SELECT 1 FROM tenant_keys WHERE tenant_id = ?1 AND label = ?2")?
                .exists(params![tenant_id, label])?;
            if taken {
                return Err(AppError::Structured {
                    code: "label_taken",
                    message: format!("this tenant already has a sub-key labelled '{label}'"),
                    data: json!({"label": label}),
                });
            }
            conn.execute(
                "INSERT INTO tenant_keys (tenant_id, key_hash, label, tools_json, created_at, expires_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![tenant_id, key_hash, label, json!(tools).to_string(), created_at, expires_at],
            )?;
            Ok(conn.last_insert_rowid())
        })
        .await
    }

    pub async fn find_sub_key_by_hash(&self, key_hash: String) -> Result<Option<SubKey>, AppError> {
        self.with_conn(move |conn| {
            let sql = format!("SELECT {SUB_KEY_COLUMNS} FROM tenant_keys WHERE key_hash = ?1");
            conn.query_row(&sql, params![key_hash], sub_key_from_row)
                .optional()
                .map_err(AppError::from)
        })
        .await
    }

    /// `last_used_at`, written at most once per minute per key.
    pub async fn touch_sub_key(&self, id: i64, now: i64) -> Result<(), AppError> {
        self.with_conn(move |conn| {
            conn.execute(
                "UPDATE tenant_keys SET last_used_at = ?2 \
                 WHERE id = ?1 AND (last_used_at IS NULL OR last_used_at <= ?2 - 60)",
                params![id, now],
            )?;
            Ok(())
        })
        .await
    }

    /// Every sub-key of `tenant_id`, oldest first.
    pub async fn list_sub_keys(&self, tenant_id: i64) -> Result<Vec<SubKey>, AppError> {
        self.with_conn(move |conn| {
            let sql = format!("SELECT {SUB_KEY_COLUMNS} FROM tenant_keys WHERE tenant_id = ?1 ORDER BY id");
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map(params![tenant_id], sub_key_from_row)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// Stamp `revoked_at` on the tenant's sub-key `label` (a no-op that keeps
    /// the first stamp when already revoked). `Ok(None)` when no such label.
    pub async fn revoke_sub_key(&self, tenant_id: i64, label: String, now: i64) -> Result<Option<i64>, AppError> {
        self.with_conn(move |conn| {
            conn.execute(
                "UPDATE tenant_keys SET revoked_at = ?3 \
                 WHERE tenant_id = ?1 AND label = ?2 AND revoked_at IS NULL",
                params![tenant_id, label, now],
            )?;
            Ok(conn
                .query_row(
                    "SELECT revoked_at FROM tenant_keys WHERE tenant_id = ?1 AND label = ?2",
                    params![tenant_id, label],
                    |r| r.get::<_, Option<i64>>(0),
                )
                .optional()?
                .flatten())
        })
        .await
    }

    /// Sub-keys of `tenant_id` that are neither revoked nor expired at `now`.
    pub async fn count_active_sub_keys(&self, tenant_id: i64, now: i64) -> Result<i64, AppError> {
        self.with_conn(move |conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM tenant_keys \
                 WHERE tenant_id = ?1 AND revoked_at IS NULL AND expires_at > ?2",
                params![tenant_id, now],
                |r| r.get(0),
            )?)
        })
        .await
    }

    pub async fn count_sub_keys(&self, tenant_id: i64) -> Result<i64, AppError> {
        self.with_conn(move |conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM tenant_keys WHERE tenant_id = ?1",
                params![tenant_id],
                |r| r.get(0),
            )?)
        })
        .await
    }
}

/// `host.key.mint {tools, ttl_s, label}` (R2): primary key only -- the
/// dispatcher refuses a sub-key caller before this runs.
pub async fn mint(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let tools: Vec<String> = match args.get("tools").and_then(Value::as_array) {
        Some(a) if !a.is_empty() && a.len() <= TOOLS_MAX => a
            .iter()
            .map(|v| v.as_str().map(str::to_string))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| AppError::InvalidParams("tools: every entry must be a tool name string".into()))?,
        _ => {
            return Err(AppError::InvalidParams(format!(
                "tools: an array of 1..{TOOLS_MAX} tool names is required"
            )));
        }
    };
    let ttl_s = match args.get("ttl_s") {
        None | Some(Value::Null) => TTL_DEFAULT_S,
        Some(v) => v
            .as_i64()
            .filter(|t| (TTL_MIN_S..=TTL_MAX_S).contains(t))
            .ok_or_else(|| {
                AppError::InvalidParams(format!("ttl_s: an integer in {TTL_MIN_S}..{TTL_MAX_S} seconds"))
            })?,
    };
    // R2: every name must resolve -- an own tool by local name, or a
    // shared-in tool by its qualified `<owner_ns>.<name>` -- before any row
    // is written; the first missing one is named.
    let own: std::collections::HashSet<String> = state
        .db
        .list_tools(tenant.id)
        .await?
        .into_iter()
        .map(|row| row.name)
        .collect();
    for name in &tools {
        let found = if own.contains(name) {
            true
        } else {
            match name.split_once('.') {
                Some((ns, local)) if ns == tenant.namespace => own.contains(local),
                Some((ns, local)) => crate::sharing::resolve_shared_tool(state, tenant, ns, local).await.is_ok(),
                None => false,
            }
        };
        if !found {
            return Err(AppError::Structured {
                code: "not_found",
                message: format!("tool '{name}' is not one of your tools or shared with you; no sub-key was minted"),
                data: json!({"tool": name}),
            });
        }
    }
    // R7: the plan's `sub_keys_max` bounds active sub-keys.
    let plan = state.plans.get(&tenant.plan).ok_or_else(|| {
        AppError::Internal(format!("tenant's plan '{}' is not in the loaded plan catalog", tenant.plan))
    })?;
    let active = state.db.count_active_sub_keys(tenant.id, now_unix()).await?;
    if active >= plan.sub_keys_max {
        return Err(AppError::Structured {
            code: "plan_required",
            message: format!(
                "sub_keys_max: the {} plan allows {} active sub-keys and this tenant holds {active} -- \
                 revoke one with host.key.revoke, or upgrade",
                tenant.plan, plan.sub_keys_max
            ),
            data: json!({
                "field": "sub_keys_max",
                "plan": tenant.plan,
                "limit": plan.sub_keys_max,
                "used": active,
            }),
        });
    }
    let key = generate_sub_key();
    let label = match args.get("label") {
        None | Some(Value::Null) => format!("key-{}", &key[3..11]),
        Some(v) => v
            .as_str()
            .filter(|l| !l.is_empty() && l.len() <= LABEL_MAX)
            .map(str::to_string)
            .ok_or_else(|| AppError::InvalidParams(format!("label: a string of 1..{LABEL_MAX} characters")))?,
    };
    let now = now_unix();
    let expires_at = now + ttl_s;
    let new = NewSubKey {
        key_hash: hash_key(&key),
        label: label.clone(),
        tools: tools.clone(),
        created_at: now,
        expires_at,
    };
    state.db.insert_sub_key(tenant.id, new).await?;
    Ok(json!({
        "key": key,
        "label": label,
        "tools": tools,
        "expires_at": expires_at,
        "usage": "Hand this key to the other agent: pass it as the tenant_key argument or an \
            Authorization: Bearer header. It is shown once. It opens only the named tools \
            (plus host.whoami and host.quickstart) until expires_at, and survives host.key.rotate.",
    }))
}

/// R4: the host tools a sub-key always sees and may call, whatever its
/// allowlist. Everything else under `host.*`/`billing.*`/`admin.*` -- the
/// always-refused set (`host.key.*`, `host.secret.*`, `host.lockdown`,
/// `host.self.offboard`, `host.export` included) -- is `permission_denied`.
pub const ALWAYS_LISTED: &[&str] = &["host.whoami", "host.quickstart", "host.tool.reach"];

impl SubKey {
    /// Whether this sub-key may see/call the tool whose wire name is `name`
    /// (own tools are `<tenant_ns>.<local>`; the allowlist holds the local
    /// name for an own tool and the qualified name for a shared-in one).
    pub fn permits(&self, tenant_ns: &str, name: &str) -> bool {
        if ALWAYS_LISTED.contains(&name) {
            return true;
        }
        let local = name.strip_prefix(tenant_ns).and_then(|r| r.strip_prefix('.'));
        self.tools.iter().any(|t| t == name || Some(t.as_str()) == local)
    }

    /// The `permission_denied` refusal for a call outside the allowlist.
    pub fn denied(&self, name: &str) -> AppError {
        AppError::Structured {
            code: "permission_denied",
            message: format!(
                "this sub-key ('{}') may call only: {}",
                self.label,
                self.tools.join(", ")
            ),
            data: json!({
                "allowed_tools": self.tools,
                "key_label": self.label,
                "tool": name,
            }),
        }
    }
}

impl SubKey {
    /// The refusal for a sub-key that can no longer authenticate, `None`
    /// while it is live (R3). Revocation wins over expiry.
    pub fn dead_reason(&self, now: i64) -> Option<AppError> {
        if let Some(at) = self.revoked_at {
            return Some(AppError::Structured {
                code: "key_revoked",
                message: format!("sub-key '{}' was revoked", self.label),
                data: json!({"key_label": self.label, "revoked_at": at}),
            });
        }
        if now >= self.expires_at {
            return Some(AppError::Structured {
                code: "key_expired",
                message: format!("sub-key '{}' expired", self.label),
                data: json!({"key_label": self.label, "expires_at": self.expires_at}),
            });
        }
        None
    }
}

/// `host.key.list` (R5): every sub-key's metadata, never the key itself.
pub async fn list(state: &AppState, tenant: &Tenant) -> Result<Value, AppError> {
    let keys: Vec<Value> = state
        .db
        .list_sub_keys(tenant.id)
        .await?
        .into_iter()
        .map(|k| {
            json!({
                "label": k.label,
                "tools": k.tools,
                "created_at": k.created_at,
                "expires_at": k.expires_at,
                "revoked_at": k.revoked_at,
                "last_used_at": k.last_used_at,
            })
        })
        .collect();
    Ok(json!({"keys": keys}))
}

/// `host.key.revoke {label}` (R5): the very next call with that sub-key
/// fails `key_revoked` -- resolution reads `revoked_at` on every request.
pub async fn revoke(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let label = args
        .get("label")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::InvalidParams("label: the sub-key label to revoke is required".into()))?;
    match state.db.revoke_sub_key(tenant.id, label.to_string(), now_unix()).await? {
        Some(revoked_at) => Ok(json!({"label": label, "revoked_at": revoked_at})),
        None => Err(AppError::Structured {
            code: "not_found",
            message: format!("no sub-key labelled '{label}'"),
            data: json!({"label": label}),
        }),
    }
}
