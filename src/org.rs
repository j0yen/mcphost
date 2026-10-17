//! `host.org.*` (PRD-mcphost-org-model): an org owns tenants, one key per
//! human, roles `owner` and `member`. Membership is a join row
//! (`org_members`) and nothing else -- no `tenants` column is rewritten, so
//! a tenant that joins keeps its key, namespace, URL and tools byte for
//! byte. Same shape as `invites.rs`: `AppState` + arguments in,
//! `serde_json::Value` (or [`AppError`]) out; `handler.rs` owns the wire.

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

use crate::db::{Db, Tenant};
use crate::errors::AppError;
use crate::state::{AppState, now_unix};

/// The one source of org roles. `migrations/0116_orgs.sql`'s CHECK list and
/// every `host.org.*` descriptor's `role` enum must equal this; the drift
/// test (tests/orgm_ac03) fails when they separate.
pub const ROLES: &[&str] = &["owner", "member"];

/// The `host.org.*` tool names, shared by the dispatcher, the descriptors
/// and the drift test.
pub const TOOLS: &[&str] = &["host.org.create", "host.org.join", "host.org.members"];

/// `ROLES` as a SQL `IN (...)` list body: `'owner', 'member'`.
pub fn roles_sql_list() -> String {
    ROLES.iter().map(|r| format!("'{r}'")).collect::<Vec<_>>().join(", ")
}

/// `ROLES` as the JSON enum array a descriptor's `role` property carries.
pub fn roles_enum_json() -> Value {
    json!(ROLES)
}

const NAME_MIN: usize = 3;
const NAME_MAX: usize = 32;

/// `^[a-z0-9]+(-[a-z0-9]+)*$`, 3-32 chars.
pub fn valid_org_name(name: &str) -> bool {
    (NAME_MIN..=NAME_MAX).contains(&name.len())
        && name.split('-').all(|part| {
            !part.is_empty() && part.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

/// Each plan's `org_members_max` plus the upgrade pointer, handed into the
/// join transactions so the cap is decided in the same transaction that
/// inserts the seat.
#[derive(Debug, Clone, Default)]
pub struct OrgCaps {
    pub by_plan: Vec<(String, i64)>,
    pub upgrade: String,
}

impl OrgCaps {
    pub fn from_state(state: &AppState) -> Self {
        Self {
            by_plan: state
                .plans
                .plans
                .iter()
                .map(|p| (p.name.clone(), p.org_members_max))
                .collect(),
            upgrade: format!("{}/plans.html", state.public_url.trim_end_matches('/')),
        }
    }
}

fn org_members_max_error(limit: i64, plan: &str, upgrade: &str) -> AppError {
    AppError::Structured {
        code: "org_members_max",
        message: format!("this org already has {limit} members, the {plan} plan maximum"),
        data: json!({"limit": limit, "plan": plan, "upgrade": upgrade}),
    }
}

/// A tenant's live membership: `(org_id, org name, role, joined_unix)`.
pub type Membership = (i64, String, String, i64);

pub fn membership_sync(
    conn: &Connection,
    tenant_id: i64,
) -> rusqlite::Result<Option<Membership>> {
    conn.query_row(
        "SELECT o.id, o.name, m.role, m.joined_unix FROM org_members m \
         JOIN orgs o ON o.id = m.org_id WHERE m.tenant_id = ?1 AND m.removed_unix IS NULL",
        params![tenant_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )
    .optional()
}

/// Refuse with `org_members_max` when the org's live seats (owner
/// included) already equal the owner plan's cap. Runs on the caller's
/// connection so it sits inside the join transaction.
pub(crate) fn check_cap_sync(
    conn: &Connection,
    org_id: Option<i64>,
    caps: &OrgCaps,
) -> Result<(), AppError> {
    let org_id = org_id.ok_or_else(AppError::invite_invalid)?;
    let plan: String = conn.query_row(
        "SELECT t.plan FROM orgs o JOIN tenants t ON t.id = o.owner_tenant_id WHERE o.id = ?1",
        params![org_id],
        |r| r.get(0),
    )?;
    let live: i64 = conn.query_row(
        "SELECT COUNT(*) FROM org_members WHERE org_id = ?1 AND removed_unix IS NULL",
        params![org_id],
        |r| r.get(0),
    )?;
    let limit = caps.by_plan.iter().find(|(n, _)| *n == plan).map(|(_, l)| *l);
    match limit {
        Some(limit) if live >= limit => Err(org_members_max_error(limit, &plan, &caps.upgrade)),
        _ => Ok(()),
    }
}

/// One membership row. A tenant removed earlier may be re-admitted (the
/// `tenant_id UNIQUE` row is revived); a live member is refused with
/// `org_member_already` naming its org.
pub(crate) fn insert_member_sync(
    conn: &Connection,
    org_id: Option<i64>,
    tenant_id: i64,
    role: &str,
    now: i64,
) -> Result<(), AppError> {
    debug_assert!(ROLES.contains(&role));
    let org_id = org_id.ok_or_else(AppError::invite_invalid)?;
    if let Some((_, name, _, _)) = membership_sync(conn, tenant_id)? {
        return Err(AppError::Structured {
            code: "org_member_already",
            message: format!("this tenant already belongs to org '{name}'"),
            data: json!({"org": name}),
        });
    }
    conn.execute(
        "INSERT INTO org_members (org_id, tenant_id, role, joined_unix) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(tenant_id) DO UPDATE SET org_id = excluded.org_id, role = excluded.role, \
         joined_unix = excluded.joined_unix, removed_unix = NULL",
        params![org_id, tenant_id, role, now],
    )?;
    Ok(())
}

/// One row of `host.org.members`.
pub struct MemberRow {
    pub namespace: String,
    pub role: String,
    pub joined_unix: i64,
    pub last_seen_unix: Option<i64>,
    pub tools: i64,
}

impl Db {
    /// `host.org.create`: org row, the owner's membership and the org invite,
    /// in one transaction.
    pub async fn create_org(
        &self,
        owner_tenant_id: i64,
        name: String,
        code_hash: String,
        code_plain: String,
    ) -> Result<(), AppError> {
        let now = now_unix();
        self.with_conn(move |conn| {
            conn.execute("BEGIN IMMEDIATE", [])?;
            let outcome: Result<(), AppError> = (|| {
                if let Some((_, current, _, _)) = membership_sync(conn, owner_tenant_id)? {
                    return Err(AppError::Structured {
                        code: "org_member_already",
                        message: format!("this tenant already belongs to org '{current}'"),
                        data: json!({"org": current}),
                    });
                }
                let taken: bool = conn
                    .prepare("SELECT 1 FROM orgs WHERE name = ?1")?
                    .exists(params![name])?;
                if taken {
                    return Err(AppError::Structured {
                        code: "org_name_taken",
                        message: format!("org name '{name}' is already taken"),
                        data: json!({"name": name}),
                    });
                }
                conn.execute(
                    "INSERT INTO orgs (name, owner_tenant_id, created_unix) VALUES (?1, ?2, ?3)",
                    params![name, owner_tenant_id, now],
                )?;
                let org_id = conn.last_insert_rowid();
                insert_member_sync(conn, Some(org_id), owner_tenant_id, "owner", now)?;
                conn.execute(
                    "INSERT INTO invites (code_hash, inviter_tenant_id, kind, share_json, \
                     created_unix, code_plain, org_id) VALUES (?1, ?2, 'org', '[]', ?3, ?4, ?5)",
                    params![code_hash, owner_tenant_id, now, code_plain, org_id],
                )?;
                Ok(())
            })();
            match outcome {
                Ok(()) => conn.execute("COMMIT", []).map(|_| ()).map_err(AppError::from),
                Err(e) => {
                    let _ = conn.execute("ROLLBACK", []);
                    Err(e)
                }
            }
        })
        .await
    }

    /// `host.org.join`: an existing tenant takes a seat; nothing about the
    /// tenant itself is written.
    pub async fn join_org(
        &self,
        tenant_id: i64,
        code_hash: String,
        caps: OrgCaps,
    ) -> Result<String, AppError> {
        let now = now_unix();
        self.with_conn(move |conn| {
            conn.execute("BEGIN IMMEDIATE", [])?;
            let outcome: Result<String, AppError> = (|| {
                let (org_id, name): (i64, String) = conn
                    .query_row(
                        "SELECT o.id, o.name FROM invites i JOIN orgs o ON o.id = i.org_id \
                         WHERE i.code_hash = ?1 AND i.kind = 'org' AND i.revoked_unix IS NULL \
                         AND (i.expires_unix IS NULL OR i.expires_unix > ?2)",
                        params![code_hash, now],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?
                    .ok_or_else(AppError::invite_invalid)?;
                if membership_sync(conn, tenant_id)?.is_none() {
                    check_cap_sync(conn, Some(org_id), &caps)?;
                }
                insert_member_sync(conn, Some(org_id), tenant_id, "member", now)?;
                Ok(name)
            })();
            match outcome {
                Ok(name) => conn.execute("COMMIT", []).map(|_| name).map_err(AppError::from),
                Err(e) => {
                    let _ = conn.execute("ROLLBACK", []);
                    Err(e)
                }
            }
        })
        .await
    }

    /// The caller's live org: `(org_id, name, role, joined_unix)`.
    pub async fn org_membership(
        &self,
        tenant_id: i64,
    ) -> Result<Option<Membership>, AppError> {
        self.with_conn(move |conn| membership_sync(conn, tenant_id).map_err(AppError::from))
            .await
    }

    /// Live members of `org_id`, oldest first, with the owner's plain
    /// per-member tool count.
    pub async fn org_members(&self, org_id: i64) -> Result<Vec<MemberRow>, AppError> {
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT t.namespace, m.role, m.joined_unix, t.last_seen_unix, \
                 (SELECT COUNT(*) FROM tools x WHERE x.tenant_id = t.id) \
                 FROM org_members m JOIN tenants t ON t.id = m.tenant_id \
                 WHERE m.org_id = ?1 AND m.removed_unix IS NULL \
                 ORDER BY m.joined_unix, t.id",
            )?;
            let rows = stmt
                .query_map(params![org_id], |r| {
                    Ok(MemberRow {
                        namespace: r.get(0)?,
                        role: r.get(1)?,
                        joined_unix: r.get(2)?,
                        last_seen_unix: r.get(3)?,
                        tools: r.get(4)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// The plaintext org invite code, `None` when no live org invite exists.
    pub async fn org_invite_code(&self, org_id: i64) -> Result<Option<String>, AppError> {
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT code_plain FROM invites WHERE org_id = ?1 AND kind = 'org' \
                 AND revoked_unix IS NULL ORDER BY id DESC LIMIT 1",
                params![org_id],
                |r| r.get(0),
            )
            .optional()
            .map(|c| c.flatten())
            .map_err(AppError::from)
        })
        .await
    }
}

fn arg_str(args: &Value, name: &str) -> Result<String, AppError> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppError::InvalidArgs(format!("missing required argument '{name}'")))
}

pub fn org_url(state: &AppState, code: &str) -> String {
    format!("{}/o/{code}/mcp", state.public_url.trim_end_matches('/'))
}

/// The `<code>` segment of a `/o/{code}/mcp` request.
pub fn org_path_code(parts: &http::request::Parts) -> Option<&str> {
    parts.uri.path().strip_prefix("/o/")?.strip_suffix("/mcp")
}

/// `host.org.create {name}`: the caller becomes owner and gets the org
/// invite URL to hand to teammates.
pub async fn create(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let name = arg_str(args, "name")?;
    if !valid_org_name(&name) {
        return Err(AppError::InvalidArgs(format!(
            "org name must match ^[a-z0-9]+(-[a-z0-9]+)*$ and be {NAME_MIN}-{NAME_MAX} characters"
        )));
    }
    let code = crate::auth::generate_url_secret();
    state
        .db
        .create_org(tenant.id, name.clone(), crate::auth::hash_key(&code), code.clone())
        .await?;
    Ok(json!({"org": name, "role": "owner", "invite_url": org_url(state, &code)}))
}

/// `host.org.join {code}`: an existing tenant joins with its own key.
pub async fn join(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let code = arg_str(args, "code")?;
    let name = state
        .db
        .join_org(tenant.id, crate::auth::hash_key(&code), OrgCaps::from_state(state))
        .await?;
    Ok(json!({"org": name, "role": "member"}))
}

/// `host.org.members {role?}`: every live member, for the owner and for
/// members alike.
pub async fn members(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let Some((org_id, name, role, _)) = state.db.org_membership(tenant.id).await? else {
        return Err(AppError::Structured {
            code: "not_in_org",
            message: "this tenant does not belong to an org".to_string(),
            data: json!({}),
        });
    };
    let filter = args.get("role").and_then(Value::as_str);
    if let Some(f) = filter
        && !ROLES.contains(&f)
    {
        return Err(AppError::InvalidArgs(format!("role must be one of {ROLES:?}")));
    }
    let members: Vec<Value> = state
        .db
        .org_members(org_id)
        .await?
        .into_iter()
        .filter(|m| filter.is_none_or(|f| f == m.role))
        .map(|m| {
            json!({
                "namespace": m.namespace,
                "role": m.role,
                "joined_at": m.joined_unix,
                "last_seen": m.last_seen_unix,
                "tools": m.tools,
            })
        })
        .collect();
    let mut out = json!({"org": name, "role": role, "members": members});
    if role == "owner"
        && let Some(code) = state.db.org_invite_code(org_id).await?
        && let Some(obj) = out.as_object_mut()
    {
        obj.insert("invite_url".to_string(), json!(org_url(state, &code)));
    }
    Ok(out)
}

/// `host.whoami`'s `org` value: `{name, role, joined_at}` or null.
pub async fn whoami_org(state: &AppState, tenant_id: i64) -> Result<Value, AppError> {
    Ok(match state.db.org_membership(tenant_id).await? {
        Some((_, name, role, joined)) => json!({"name": name, "role": role, "joined_at": joined}),
        None => Value::Null,
    })
}
