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

/// PRD-mcphost-acting-for-in-ledger-and-audit R1: who a delegated key's call
/// acted for -- labels and the row id only, never a key or a hash.
#[derive(Clone, Debug, PartialEq)]
pub struct ActingFor {
    pub key_id: i64,
    pub label: String,
    pub parent_label: Option<String>,
    pub root_label: String,
    pub depth: i64,
}

impl ActingFor {
    /// The wire object; its five keys are the whole contract.
    pub fn to_json(&self) -> Value {
        json!({
            "key_id": self.key_id,
            "label": self.label,
            "parent_label": self.parent_label,
            "root_label": self.root_label,
            "depth": self.depth,
        })
    }
}

tokio::task_local! {
    /// The lineage of the sub-key the current `tools/call` authenticated
    /// with, resolved once (one recursive query) by `call_tool_inner` and
    /// read by the response/error decorators. `None` for the primary.
    pub static CALL_ACTING_FOR: std::cell::RefCell<Option<ActingFor>>;
}

/// The `acting_for` of the call running on this task, if a sub-key made it.
pub fn current_acting_for() -> Option<ActingFor> {
    CALL_ACTING_FOR.try_with(|c| c.borrow().clone()).ok().flatten()
}

/// The recursive lineage query behind [`Db::acting_for_for_key`], for callers
/// already holding the connection.
pub fn acting_for_sync(conn: &rusqlite::Connection, id: i64) -> Result<Option<ActingFor>, AppError> {
    let mut stmt = conn.prepare(
        "WITH RECURSIVE up(id, label, parent_key_id, depth, lvl) AS (\
           SELECT id, label, parent_key_id, depth, 0 FROM tenant_keys WHERE id = ?1 \
           UNION ALL \
           SELECT k.id, k.label, k.parent_key_id, k.depth, up.lvl + 1 \
           FROM tenant_keys k JOIN up ON k.id = up.parent_key_id) \
         SELECT id, label, depth FROM up ORDER BY lvl",
    )?;
    let chain = stmt
        .query_map(params![id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    let Some((key_id, label, depth)) = chain.first().cloned() else { return Ok(None) };
    let parent_label = chain.get(1).map(|c| c.1.clone());
    let root_label = chain.last().map(|c| c.1.clone()).unwrap_or_else(|| label.clone());
    Ok(Some(ActingFor { key_id, label, parent_label, root_label, depth }))
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
    /// PRD-mcphost-key-delegate-child-keys R1: `None` = child of the primary.
    pub parent_key_id: Option<i64>,
    pub scopes: Vec<crate::scopes::Scope>,
    pub can_delegate: bool,
    pub depth: i64,
    pub revoked_by: Option<String>,
    /// PRD-mcphost-delegation-bound-to-run-or-session R1: `Some(("run" |
    /// "session", id))` ties the key to one run or one MCP connection.
    pub bound: Option<(String, String)>,
    pub bound_last_seen_unix: Option<i64>,
    /// Why a bound key died: `run_<state>`, `session_idle`, `session_closed`.
    pub revoked_reason: Option<String>,
    /// `tools_json` is `null`: no allowlist (a delegation that named no
    /// tools under a parent with none).
    pub unrestricted: bool,
}

const SUB_KEY_COLUMNS: &str = "id, tenant_id, key_hash, label, tools_json, created_at, expires_at, \
     revoked_at, last_used_at, parent_key_id, scopes_json, can_delegate, depth, revoked_by, \
     bound_kind, bound_ref, bound_last_seen_unix, revoked_reason";

fn sub_key_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<SubKey> {
    let tools_json: String = r.get(4)?;
    let scopes_json: String = r.get(10)?;
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
        parent_key_id: r.get(9)?,
        scopes: crate::scopes::parse_json(&scopes_json),
        can_delegate: r.get::<_, i64>(11)? != 0,
        depth: r.get(12)?,
        revoked_by: r.get(13)?,
        bound: match (r.get::<_, Option<String>>(14)?, r.get::<_, Option<String>>(15)?) {
            (Some(kind), Some(id)) => Some((kind, id)),
            _ => None,
        },
        bound_last_seen_unix: r.get(16)?,
        revoked_reason: r.get(17)?,
        unrestricted: tools_json.trim() == "null",
    })
}

/// Stamp one bound key revoked with `reason` and write its `revoke_bound`
/// audit row, on the caller's connection (so inside the caller's
/// transaction).
fn revoke_bound_key(
    conn: &rusqlite::Connection,
    (tenant_id, label): (i64, &str),
    (kind, bound_ref): (&str, &str),
    reason: &str,
    now: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE tenant_keys SET revoked_at = ?4, revoked_by = label, revoked_reason = ?5 \
         WHERE tenant_id = ?1 AND label = ?2 AND bound_kind = ?3 AND revoked_at IS NULL",
        params![tenant_id, label, kind, now, reason],
    )?;
    conn.execute(
        "INSERT INTO tenant_audit (tenant_id, subject, action, detail, created_unix) \
         VALUES (?1, ?2, 'revoke_bound', ?3, ?4)",
        params![
            tenant_id,
            format!("key:{label}"),
            json!({"label": label, "bound": {"kind": kind, "ref": bound_ref}, "reason": reason}).to_string(),
            now
        ],
    )?;
    Ok(())
}

/// R4/R5: revoke every live key of `tenant_id` bound to `(kind, bound_ref)`
/// with `reason`, one `revoke_bound` audit row each. Runs inside the
/// caller's transaction so the run's terminal write and the revocation
/// commit together. Returns how many keys it revoked.
pub(crate) fn revoke_bound_on(
    conn: &rusqlite::Connection,
    tenant_id: i64,
    bound: (&str, &str),
    reason: &str,
    now: i64,
) -> rusqlite::Result<usize> {
    let labels: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT label FROM tenant_keys WHERE tenant_id = ?1 AND bound_kind = ?2 AND bound_ref = ?3 \
             AND revoked_at IS NULL ORDER BY id",
        )?;
        stmt.query_map(params![tenant_id, bound.0, bound.1], |r| r.get(0))?.collect::<Result<_, _>>()?
    };
    for label in &labels {
        revoke_bound_key(conn, (tenant_id, label), bound, reason, now)?;
    }
    Ok(labels.len())
}

/// R5: a session-bound key idle this long is swept.
pub const SESSION_IDLE_S: i64 = 30 * 60;
/// R5: `bound_last_seen_unix` is written at most this often, and the idle
/// sweep runs this often.
pub const SESSION_TOUCH_S: i64 = 60;

impl Db {
    /// R5: `bound_last_seen_unix`, written at most once per minute.
    pub async fn touch_bound_session(&self, id: i64, now: i64) -> Result<(), AppError> {
        self.with_conn(move |conn| {
            conn.execute(
                "UPDATE tenant_keys SET bound_last_seen_unix = ?2 WHERE id = ?1 AND bound_kind = 'session' \
                 AND revoked_at IS NULL AND (bound_last_seen_unix IS NULL OR bound_last_seen_unix <= ?2 - ?3)",
                params![id, now, SESSION_TOUCH_S],
            )?;
            Ok(())
        })
        .await
    }

    /// R5: the MCP session `session_id` ended -- every key bound to it dies
    /// now (`session_closed`). Returns how many.
    pub async fn revoke_session_bound(&self, session_id: String, now: i64) -> Result<usize, AppError> {
        self.with_conn(move |conn| {
            let tx = conn.unchecked_transaction()?;
            let tenants: Vec<i64> = {
                let mut stmt = tx.prepare(
                    "SELECT DISTINCT tenant_id FROM tenant_keys WHERE bound_kind = 'session' AND bound_ref = ?1 \
                     AND revoked_at IS NULL",
                )?;
                stmt.query_map(params![session_id], |r| r.get(0))?.collect::<Result<_, _>>()?
            };
            let mut n = 0;
            for tenant_id in tenants {
                n += revoke_bound_on(&tx, tenant_id, ("session", &session_id), "session_closed", now)?;
            }
            tx.commit()?;
            Ok(n)
        })
        .await
    }

    /// R5: revoke every session-bound key whose session has been silent for
    /// more than [`SESSION_IDLE_S`] as of `now` (`session_idle`). One query
    /// per tick. Returns how many.
    pub async fn sweep_idle_bound_sessions(&self, now: i64) -> Result<usize, AppError> {
        self.with_conn(move |conn| {
            let tx = conn.unchecked_transaction()?;
            let idle: Vec<(i64, String, String)> = {
                let mut stmt = tx.prepare(
                    "SELECT tenant_id, label, bound_ref FROM tenant_keys WHERE bound_kind = 'session' \
                     AND revoked_at IS NULL AND COALESCE(bound_last_seen_unix, created_at) < ?1 - ?2",
                )?;
                stmt.query_map(params![now, SESSION_IDLE_S], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                    .collect::<Result<_, _>>()?
            };
            for (tenant_id, label, session_id) in &idle {
                revoke_bound_key(&tx, (*tenant_id, label), ("session", session_id), "session_idle", now)?;
            }
            tx.commit()?;
            Ok(idle.len())
        })
        .await
    }
}

/// R5: the idle sweep, every [`SESSION_TOUCH_S`]; started once at `serve`
/// startup like the other background ticks, never joined.
pub fn spawn_bound_session_sweep(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(SESSION_TOUCH_S as u64)).await;
            if let Err(e) = state.db.sweep_idle_bound_sessions(now_unix()).await {
                tracing::warn!(error = %e, "bound-session idle sweep failed");
            }
        }
    })
}

/// What [`Db::revoke_sub_key_tree`] stamped.
pub struct Revoked {
    pub revoked_at: i64,
    pub labels: Vec<String>,
}

/// The columns [`Db::insert_sub_key`] writes.
pub struct NewSubKey {
    pub key_hash: String,
    pub label: String,
    pub tools: Vec<String>,
    pub created_at: i64,
    pub expires_at: i64,
    pub parent_key_id: Option<i64>,
    pub scopes: Vec<crate::scopes::Scope>,
    pub can_delegate: bool,
    pub depth: i64,
    /// `None` stores `null`: no allowlist.
    pub unrestricted: bool,
    /// `(kind, ref)` of a run- or session-bound key.
    pub bound: Option<(String, String)>,
}

impl Db {
    /// Insert one sub-key; a label already used by this tenant is
    /// `label_taken`.
    pub async fn insert_sub_key(&self, tenant_id: i64, new: NewSubKey) -> Result<i64, AppError> {
        let NewSubKey { key_hash, label, tools, created_at, expires_at, parent_key_id, scopes, can_delegate, depth, unrestricted, bound } =
            new;
        let (bound_kind, bound_ref) = bound.unzip();
        let bound_seen = bound_kind.as_ref().map(|_| created_at);
        let tools_json = if unrestricted { "null".to_string() } else { json!(tools).to_string() };
        let scopes_json = crate::delegation::scopes_json(&scopes).to_string();
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
                "INSERT INTO tenant_keys (tenant_id, key_hash, label, tools_json, created_at, expires_at, \
                 parent_key_id, scopes_json, can_delegate, depth, bound_kind, bound_ref, bound_last_seen_unix) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    tenant_id, key_hash, label, tools_json, created_at, expires_at, parent_key_id, scopes_json,
                    can_delegate as i64, depth, bound_kind, bound_ref, bound_seen
                ],
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

    /// R1: the `acting_for` lineage of key `id`, rendered by one recursive
    /// query over `tenant_keys` (key, parent, ... root).
    pub async fn acting_for_for_key(&self, id: i64) -> Result<Option<ActingFor>, AppError> {
        self.with_conn(move |conn| acting_for_sync(conn, id)).await
    }

    pub async fn find_sub_key_by_id(&self, id: i64) -> Result<Option<SubKey>, AppError> {
        self.with_conn(move |conn| {
            let sql = format!("SELECT {SUB_KEY_COLUMNS} FROM tenant_keys WHERE id = ?1");
            conn.query_row(&sql, params![id], sub_key_from_row).optional().map_err(AppError::from)
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

    /// R4: revoke `label` -- and, with `cascade`, every descendant -- in one
    /// transaction (a recursive CTE walks the subtree). A key already revoked
    /// keeps its first stamp. `Ok(None)` when no such label; without
    /// `cascade`, live descendants fail `has_children {count}` and nothing
    /// changes.
    pub async fn revoke_sub_key_tree(
        &self,
        tenant_id: i64,
        label: String,
        cascade: bool,
        now: i64,
    ) -> Result<Option<Revoked>, AppError> {
        const SUBTREE: &str = "WITH RECURSIVE sub(id) AS (SELECT id FROM tenant_keys WHERE id = ?1 \
             UNION ALL SELECT k.id FROM tenant_keys k JOIN sub ON k.parent_key_id = sub.id)";
        self.with_conn(move |conn| {
            let tx = conn.unchecked_transaction()?;
            let Some((id, stamp)) = tx
                .query_row(
                    "SELECT id, revoked_at FROM tenant_keys WHERE tenant_id = ?1 AND label = ?2",
                    params![tenant_id, label],
                    |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?)),
                )
                .optional()?
            else {
                return Ok(None);
            };
            if !cascade {
                let live: i64 = tx.query_row(
                    &format!(
                        "{SUBTREE} SELECT COUNT(*) FROM tenant_keys WHERE id IN (SELECT id FROM sub) \
                         AND id != ?1 AND revoked_at IS NULL"
                    ),
                    params![id],
                    |r| r.get(0),
                )?;
                if live > 0 {
                    return Err(AppError::Structured {
                        code: "has_children",
                        message: format!(
                            "sub-key '{label}' has {live} live descendant(s); pass cascade: true to revoke them too"
                        ),
                        data: json!({"label": label, "count": live}),
                    });
                }
            }
            let filter = if cascade { "id IN (SELECT id FROM sub)" } else { "id = ?1" };
            let labels: Vec<String> = {
                let mut stmt = tx.prepare(&format!(
                    "{SUBTREE} SELECT label FROM tenant_keys WHERE {filter} AND revoked_at IS NULL ORDER BY depth, id"
                ))?;
                stmt.query_map(params![id], |r| r.get(0))?.collect::<Result<Vec<_>, _>>()?
            };
            tx.execute(
                &format!(
                    "{SUBTREE} UPDATE tenant_keys SET revoked_at = ?2, revoked_by = ?3 \
                     WHERE {filter} AND revoked_at IS NULL"
                ),
                params![id, now, label],
            )?;
            tx.commit()?;
            Ok(Some(Revoked { revoked_at: stamp.unwrap_or(now), labels }))
        })
        .await
    }

    /// Keys of `tenant_id` that are neither revoked nor expired at `now`.
    pub async fn count_live_sub_keys(&self, tenant_id: i64, now: i64) -> Result<i64, AppError> {
        self.with_conn(move |conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM tenant_keys WHERE tenant_id = ?1 AND revoked_at IS NULL AND expires_at > ?2",
                params![tenant_id, now],
                |r| r.get(0),
            )?)
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
        parent_key_id: None,
        scopes: vec![crate::scopes::Scope::Mcp],
        can_delegate: false,
        depth: 1,
        unrestricted: false,
        bound: None,
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
pub const ALWAYS_LISTED: &[&str] = &["host.whoami", "host.quickstart", "host.tool.reach", "host.calls.list"];

/// Prefixes no sub-key reaches even without an allowlist.
const ALWAYS_REFUSED: &[&str] =
    &["host.key.", "host.secret.", "host.lockdown", "host.self.offboard", "host.export"];

impl SubKey {
    /// Whether this sub-key may see/call the tool whose wire name is `name`
    /// (own tools are `<tenant_ns>.<local>`; the allowlist holds the local
    /// name for an own tool and the qualified name for a shared-in one).
    pub fn permits(&self, tenant_ns: &str, name: &str) -> bool {
        if ALWAYS_LISTED.contains(&name) {
            return true;
        }
        if self.unrestricted {
            return !(name.starts_with("admin.") || ALWAYS_REFUSED.iter().any(|p| name.starts_with(p)));
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
    /// R6: `{kind, ref, last_seen_unix}` for a bound key, `null` otherwise.
    pub fn bound_json(&self) -> Value {
        match &self.bound {
            Some((kind, id)) => json!({"kind": kind, "ref": id, "last_seen_unix": self.bound_last_seen_unix}),
            None => Value::Null,
        }
    }

    /// R3: a session-bound key is valid only on the connection whose
    /// `Mcp-Session-Id` equals `bound_ref`, a run-bound one only on a call
    /// that carries that `run_id`; anywhere else it is
    /// `key_bound_elsewhere {bound, seen}`.
    pub fn check_bound(&self, session_id: Option<&str>, run_id: Option<&str>) -> Result<(), AppError> {
        let Some((kind, id)) = &self.bound else { return Ok(()) };
        let seen = if kind == "run" { run_id } else { session_id };
        if seen == Some(id.as_str()) {
            return Ok(());
        }
        Err(AppError::Structured {
            code: "key_bound_elsewhere",
            message: format!("sub-key '{}' is bound to {kind} {id} and cannot be used here", self.label),
            data: json!({
                "bound": {"kind": kind, "ref": id},
                "seen": {"kind": kind, "ref": seen},
                "key_label": self.label,
            }),
        })
    }

    /// The refusal for a sub-key that can no longer authenticate, `None`
    /// while it is live (R3). Revocation wins over expiry.
    pub fn dead_reason(&self, now: i64) -> Option<AppError> {
        if let Some(at) = self.revoked_at {
            return Some(AppError::Structured {
                code: "key_revoked",
                message: format!("sub-key '{}' was revoked", self.label),
                data: json!({
                    "key_label": self.label,
                    "revoked_at": at,
                    "revoked_by": self.revoked_by.as_deref().unwrap_or(&self.label),
                    "reason": self.revoked_reason,
                }),
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

/// R6: the wire form of one key, `children` filled by [`list`].
fn node_json(k: &SubKey, parent_label: Option<&str>) -> Value {
    json!({
        "label": k.label,
        "depth": k.depth,
        "parent": parent_label,
        "scopes": crate::delegation::scopes_json(&k.scopes),
        "tools": if k.unrestricted { Value::Null } else { json!(k.tools) },
        "can_delegate": k.can_delegate,
        "created_at": k.created_at,
        "expires_at": k.expires_at,
        "revoked_at": k.revoked_at,
        "last_used_at": k.last_used_at,
        "bound": k.bound_json(),
        "revoked_reason": k.revoked_reason,
        "children": [],
    })
}

fn attach(all: &[SubKey], of: Option<i64>, parent_label: Option<&str>) -> Vec<Value> {
    all.iter()
        .filter(|k| k.parent_key_id == of)
        .map(|k| {
            let mut node = node_json(k, parent_label);
            node["children"] = json!(attach(all, Some(k.id), Some(&k.label)));
            node
        })
        .collect()
}

/// `host.key.list` (R5/R6): every key's metadata as a tree rooted at the
/// primary's direct children, never the key itself.
pub async fn list(state: &AppState, tenant: &Tenant) -> Result<Value, AppError> {
    let all = state.db.list_sub_keys(tenant.id).await?;
    Ok(json!({"keys": attach(&all, None, None)}))
}

/// R6: the `key` object `host.whoami` adds when the caller is a sub-key.
pub async fn whoami_key(state: &AppState) -> Result<Option<Value>, AppError> {
    let Some(id) = current_key_id() else { return Ok(None) };
    let Some(k) = state.db.find_sub_key_by_id(id).await? else { return Ok(None) };
    let parent = match k.parent_key_id {
        Some(pid) => state.db.find_sub_key_by_id(pid).await?.map(|p| p.label),
        None => None,
    };
    let mut out = node_json(&k, parent.as_deref());
    if let Some(obj) = out.as_object_mut() {
        obj.remove("children");
        obj.insert("kind".into(), json!("delegated"));
    }
    Ok(Some(out))
}

/// `host.key.revoke {label, cascade?}` (R4/R5): the very next call with that
/// key -- or, cascading, any key beneath it -- fails `key_revoked` naming
/// `label` as `revoked_by`; resolution reads `revoked_at` on every request.
pub async fn revoke(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let label = args
        .get("label")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::InvalidParams("label: the sub-key label to revoke is required".into()))?;
    let cascade = match args.get("cascade") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(AppError::InvalidParams("cascade: a boolean".into())),
    };
    match state.db.revoke_sub_key_tree(tenant.id, label.to_string(), cascade, now_unix()).await? {
        Some(done) => {
            if cascade && !done.labels.is_empty() {
                // PRD-mcphost-acting-for-in-ledger-and-audit R2: one record
                // per revoked key, all under the cascade root's subject.
                for revoked in &done.labels {
                    audit_cascade_member(state, tenant, label, revoked, done.revoked_at).await;
                }
            } else if !done.labels.is_empty() {
                audit(state, tenant, label, "revoke", json!({"label": label, "cascade": cascade})).await;
            }
            Ok(json!({
                "label": label,
                "revoked_at": done.revoked_at,
                "revoked": done.labels.len(),
                "labels": done.labels,
            }))
        }
        None => Err(AppError::Structured {
            code: "not_found",
            message: format!("no sub-key labelled '{label}'"),
            data: json!({"label": label}),
        }),
    }
}

/// R2: one `revoke_cascade` record for `revoked`, in both `tenant_audit` and
/// the hash-chained `audit_chain` (subject `key:<root>`, so
/// `host.audit.chain {subject}` lists the whole cascade); best-effort.
async fn audit_cascade_member(state: &AppState, tenant: &Tenant, root: &str, revoked: &str, at: i64) {
    let detail = json!({"label": revoked, "revoked_by": root, "cascade": true});
    audit(state, tenant, root, "revoke_cascade", detail.clone()).await;
    if let Err(e) = state
        .db
        .audit_chain_append_event(
            tenant.id,
            format!("key:{root}"),
            "key".to_string(),
            String::new(),
            "revoke_cascade".to_string(),
            1,
            None,
            at,
            crate::state::new_ulid(),
            Some("revoke_cascade".to_string()),
            Some(detail.to_string()),
        )
        .await
    {
        tracing::warn!(error = %e, tenant = %tenant.namespace, "failed to chain key revoke record");
    }
}

/// R7: one `tenant_audit` row, `subject: key:<label>`; best-effort, so a
/// write failure never fails the verb it records.
async fn audit(state: &AppState, tenant: &Tenant, label: &str, action: &str, detail: Value) {
    if let Err(e) = state
        .db
        .record_tenant_audit(tenant.id, format!("key:{label}"), action.to_string(), Some(detail.to_string()))
        .await
    {
        tracing::warn!(error = %e, tenant = %tenant.namespace, "failed to record key audit row");
    }
}

impl SubKey {
    /// R3: whether this key's scopes cover the verb `name`.
    pub fn scope_ok(&self, name: &str) -> bool {
        crate::scopes::required_for(name).is_none_or(|need| crate::scopes::held_implies(&self.scopes, need))
    }

    /// The `permission_denied` refusal for a verb outside this key's scopes.
    pub fn scope_denied(&self, name: &str) -> AppError {
        let need = crate::scopes::required_for(name).map(|s| s.as_str());
        AppError::Structured {
            code: "permission_denied",
            message: format!("this key ('{}') lacks the scope {} for {name}", self.label, need.unwrap_or("-")),
            data: json!({
                "reason": "missing_scope",
                "required_scope": need,
                "scopes": crate::delegation::scopes_json(&self.scopes),
                "key_label": self.label,
                "tool": name,
            }),
        }
    }

    /// What this key may do, the parent side of the subset rule.
    pub fn authority(&self) -> crate::delegation::Authority {
        crate::delegation::Authority {
            scopes: self.scopes.clone(),
            tools: if self.unrestricted { None } else { Some(self.tools.clone()) },
            expires_at: Some(self.expires_at),
            depth: self.depth,
            can_delegate: self.can_delegate,
        }
    }
}

/// R5: a plan ceiling hit, naming the `plans.toml` field.
fn plan_required(field: &str, limit: i64, requested: i64, plan: &str) -> AppError {
    AppError::Structured {
        code: "plan_required",
        message: format!("{field} is {limit} on the {plan} plan (requested {requested}); upgrade the plan or revoke keys"),
        data: json!({"field": field, "limit": limit, "requested": requested, "plan": plan}),
    }
}

/// What `bind` resolved to: the binding, plus the run's own deadline when
/// the key is run-bound (R2: the child never outlives it).
struct Bind {
    kind: &'static str,
    id: String,
    deadline: Option<i64>,
}

/// R2: `bind: {run_id} | {session_id} | {session: "current"}`. A run must be
/// open and the tenant's own (`not_found` otherwise); a session must be the
/// caller's own connection (a foreign id is `permission_denied`).
async fn parse_bind(
    state: &AppState,
    tenant: &Tenant,
    bind: Option<&Value>,
    session_id: Option<&str>,
    now: i64,
) -> Result<Option<Bind>, AppError> {
    let bind = match bind {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Object(m)) => m,
        Some(_) => return Err(AppError::InvalidParams("bind: an object".into())),
    };
    let one_of = || {
        AppError::InvalidParams("bind: exactly one of run_id, session_id, session (\"current\")".into())
    };
    if bind.len() != 1 {
        return Err(one_of());
    }
    let current = || {
        session_id
            .map(|s| Bind { kind: "session", id: s.to_string(), deadline: None })
            .ok_or_else(|| AppError::InvalidParams("bind: this call carries no MCP session id to bind to".into()))
    };
    match bind.iter().next() {
        Some((k, v)) if k == "session" && v.as_str() == Some("current") => current().map(Some),
        Some((k, v)) if k == "session_id" => {
            let want = v
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| AppError::InvalidParams("bind.session_id: a non-empty string".into()))?;
            if session_id != Some(want) {
                return Err(AppError::Structured {
                    code: "permission_denied",
                    message: "bind.session_id must be this connection's own session id".into(),
                    data: json!({"reason": "foreign_session"}),
                });
            }
            current().map(Some)
        }
        Some((k, v)) if k == "run_id" => {
            let id = v
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| AppError::InvalidParams("bind.run_id: a non-empty string".into()))?;
            let not_found = || AppError::Structured {
                code: "not_found",
                message: format!("bind.run_id: no open run '{id}' of yours; no key was minted"),
                data: json!({"run_id": id}),
            };
            let run = state.db.get_run(id.to_string(), tenant.id).await?.ok_or_else(not_found)?;
            if !matches!(run.status.as_str(), "queued" | "running") {
                return Err(not_found());
            }
            let deadline = run.deadline_s.map(|d| run.started_unix.unwrap_or(now) + d);
            Ok(Some(Bind { kind: "run", id: id.to_string(), deadline }))
        }
        _ => Err(one_of()),
    }
}

/// `host.key.delegate {scopes, tools?, ttl_s, label?, can_delegate?}` (R2):
/// callable by the primary and by a delegated key with `can_delegate`.
pub async fn delegate(
    state: &AppState,
    tenant: &Tenant,
    args: &Value,
    session_id: Option<&str>,
) -> Result<Value, AppError> {
    let caller = match current_key_id() {
        Some(id) => state.db.find_sub_key_by_id(id).await?,
        None => None,
    };
    let parent = caller.as_ref().map(SubKey::authority).unwrap_or_else(crate::delegation::Authority::primary);
    if !parent.can_delegate {
        return Err(AppError::Structured {
            code: "permission_denied",
            message: "this key may not delegate: it was minted with can_delegate=false; ask the key that minted it".into(),
            data: json!({"reason": "can_delegate=false", "key_label": caller.as_ref().map(|c| c.label.clone())}),
        });
    }
    let now = now_unix();
    let bound = parse_bind(state, tenant, args.get("bind"), session_id, now).await?;
    // A bound key is a whole unit of work, so scopes default to `mcp`; an
    // unbound one still has to say what it may do.
    let scopes: Vec<crate::scopes::Scope> = match args.get("scopes").and_then(Value::as_array) {
        None if bound.is_some() && !matches!(args.get("scopes"), Some(v) if !v.is_null()) => {
            vec![crate::scopes::Scope::Mcp]
        }
        Some(a) if !a.is_empty() => a
            .iter()
            .map(|v| v.as_str().and_then(crate::scopes::Scope::parse))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| {
                AppError::InvalidParams(
                    "scopes: every entry must be one of mcp, tools:call, tools:write".into(),
                )
            })?,
        _ => return Err(AppError::InvalidParams("scopes: an array of 1.. scope names is required".into())),
    };
    let tools: Option<Vec<String>> = match args.get("tools") {
        None | Some(Value::Null) => None,
        Some(Value::Array(a)) if !a.is_empty() && a.len() <= TOOLS_MAX => Some(
            a.iter()
                .map(|v| v.as_str().filter(|s| !s.is_empty()).map(str::to_string))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| AppError::InvalidParams("tools: every entry must be a tool name string".into()))?,
        ),
        Some(_) => {
            return Err(AppError::InvalidParams(format!("tools: an array of 1..{TOOLS_MAX} tool names")));
        }
    };
    let ttl_s = match (args.get("ttl_s"), &bound) {
        // The key lives as long as its run (or the plan / parent allow).
        (None | Some(Value::Null), Some(b)) => {
            let plan_max = state.plans.get(&tenant.plan).map(|p| p.delegated_ttl_max_s);
            [
                Some(TTL_DEFAULT_S),
                plan_max,
                b.deadline.map(|d| d - now),
                parent.expires_at.map(|e| e - now),
            ]
            .into_iter()
            .flatten()
            .min()
            .unwrap_or(TTL_DEFAULT_S)
            .max(TTL_MIN_S)
        }
        (v, _) => v
            .and_then(Value::as_i64)
            .filter(|t| *t >= TTL_MIN_S)
            .ok_or_else(|| {
                AppError::InvalidParams(format!("ttl_s: an integer of at least {TTL_MIN_S} seconds is required"))
            })?,
    };
    let key = generate_sub_key();
    let label = match args.get("label") {
        None | Some(Value::Null) => format!("key-{}", &key[3..11]),
        Some(v) => v
            .as_str()
            .filter(|l| !l.is_empty() && l.len() <= LABEL_MAX)
            .map(str::to_string)
            .ok_or_else(|| AppError::InvalidParams(format!("label: a string of 1..{LABEL_MAX} characters")))?,
    };
    let can_delegate = match args.get("can_delegate") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(AppError::InvalidParams("can_delegate: a boolean".into())),
    };
    let req = crate::delegation::DelegateReq { scopes, tools, ttl_s, now, can_delegate };
    let child = crate::delegation::narrow(&parent, &req).map_err(crate::delegation::Excess::into_error)?;
    let mut expires_at = child.expires_at.unwrap_or(now + ttl_s);
    // R2: a run-bound key never outlives the run's own deadline.
    if let Some(deadline) = bound.as_ref().and_then(|b| b.deadline) {
        expires_at = expires_at.min(deadline);
    }
    // R5: the plan's ceilings, after the subset rule so a child that merely
    // outlives its parent is still `scope_escalation`.
    let plan = state
        .plans
        .get(&tenant.plan)
        .ok_or_else(|| AppError::Internal(format!("tenant plan '{}' not in catalog", tenant.plan)))?;
    if ttl_s > plan.delegated_ttl_max_s {
        return Err(plan_required("delegated_ttl_max_s", plan.delegated_ttl_max_s, ttl_s, &plan.name));
    }
    let live = state.db.count_live_sub_keys(tenant.id, now).await?;
    if live >= plan.delegated_keys_max {
        return Err(plan_required("delegated_keys_max", plan.delegated_keys_max, live, &plan.name));
    }
    let new = NewSubKey {
        key_hash: hash_key(&key),
        label: label.clone(),
        tools: child.tools.clone().unwrap_or_default(),
        created_at: now,
        expires_at,
        parent_key_id: caller.as_ref().map(|c| c.id),
        scopes: child.scopes.clone(),
        can_delegate,
        depth: child.depth,
        unrestricted: child.tools.is_none(),
        bound: bound.as_ref().map(|b| (b.kind.to_string(), b.id.clone())),
    };
    state.db.insert_sub_key(tenant.id, new).await?;
    let detail = json!({
        "scopes": crate::delegation::scopes_json(&child.scopes),
        "tools": child.tools,
        "ttl_s": ttl_s,
        "label": label,
        "can_delegate": can_delegate,
        "parent": caller.as_ref().map(|c| c.label.clone()),
    });
    audit(state, tenant, &label, "delegate", detail).await;
    let parent_label = caller.as_ref().map(|c| c.label.clone());
    Ok(json!({
        "key": key,
        "label": label,
        "parent": parent_label,
        "scopes": crate::delegation::scopes_json(&child.scopes),
        "tools": child.tools,
        "expires_at": expires_at,
        "depth": child.depth,
        "can_delegate": can_delegate,
        "bound": bound.as_ref().map(|b| json!({"kind": b.kind, "ref": b.id})),
        "usage": "Hand this key to the other agent: pass it as the tenant_key argument or an \
            Authorization: Bearer header. It is shown once. It can do no more than the key that \
            minted it, and revoking that key (cascade) revokes it.",
    }))
}

/// R3: the run id a request names -- the `run_id` argument of a `host.runs.*`
/// call, else `_meta.run_id` (`calls.run_id` propagation inside
/// `host.tool_call` stamps the same id on its children).
pub fn seen_run_id(
    tool: &str,
    args: &Value,
    meta: Option<&serde_json::Map<String, Value>>,
) -> Option<String> {
    let from_args = if tool.starts_with("host.runs.") { args.get("run_id").and_then(Value::as_str) } else { None };
    from_args
        .or_else(|| meta.and_then(|m| m.get("run_id")).and_then(Value::as_str))
        .map(str::to_string)
}
