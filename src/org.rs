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

/// The verb that starts an org; the reach sweep (tests/support/osreach.rs)
/// calls it to put its populated tenant in an org without spelling the name.
pub const CREATE_TOOL: &str = "host.org.create";

/// The `host.org.*` tool names, shared by the dispatcher, the descriptors
/// and the drift test.
pub const TOOLS: &[&str] = &[
    CREATE_TOOL,
    "host.org.join",
    "host.org.members",
    "host.org.remove",
    "host.org.leave",
    "host.org.usage",
    "host.org.digest.set",
    "host.org.digest.send",
    "host.org.export",
    "host.org.offboard",
    "host.org.restore",
];

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
        code: ORG_MEMBERS_MAX,
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
            code: ORG_MEMBER_ALREADY,
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

/// One live member as an owner-side job reads it: the tenant id the job
/// passes to the per-tenant builders, plus what the org manifest records.
pub struct LiveMember {
    pub tenant_id: i64,
    pub namespace: String,
    pub role: String,
    pub joined_unix: i64,
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
            crate::txn::with_tx(conn, crate::txn::TxBehavior::Immediate, |conn| {
                if let Some((_, current, _, _)) = membership_sync(conn, owner_tenant_id)? {
                    return Err(AppError::Structured {
                        code: ORG_MEMBER_ALREADY,
                        message: format!("this tenant already belongs to org '{current}'"),
                        data: json!({"org": current}),
                    });
                }
                let taken: bool = conn
                    .prepare("SELECT 1 FROM orgs WHERE name = ?1")?
                    .exists(params![name])?;
                if taken {
                    return Err(AppError::Structured {
                        code: ORG_NAME_TAKEN,
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
            })
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
            crate::txn::with_tx(conn, crate::txn::TxBehavior::Immediate, |conn| {
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
            })
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

    /// Live members of `org_id` (`removed_unix IS NULL`) with their tenant
    /// ids, oldest first.
    pub async fn org_live_members(&self, org_id: i64) -> Result<Vec<LiveMember>, AppError> {
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT t.id, t.namespace, m.role, m.joined_unix \
                 FROM org_members m JOIN tenants t ON t.id = m.tenant_id \
                 WHERE m.org_id = ?1 AND m.removed_unix IS NULL \
                 ORDER BY m.joined_unix, t.id",
            )?;
            let rows = stmt
                .query_map(params![org_id], |r| {
                    Ok(LiveMember {
                        tenant_id: r.get(0)?,
                        namespace: r.get(1)?,
                        role: r.get(2)?,
                        joined_unix: r.get(3)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// PRD-mcphost-org-shared-tools-namespace-v2 R4: whether `a` and `b` are
    /// live members of the same org (`org_members.tenant_id` is UNIQUE, so a
    /// tenant has at most one live row). Read per call; nothing is cached.
    pub async fn same_live_org(&self, a: i64, b: i64) -> Result<bool, AppError> {
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT 1 FROM org_members x JOIN org_members y ON y.org_id = x.org_id \
                 WHERE x.tenant_id = ?1 AND y.tenant_id = ?2 \
                 AND x.removed_unix IS NULL AND y.removed_unix IS NULL",
                params![a, b],
                |_| Ok(()),
            )
            .optional()
            .map(|r| r.is_some())
            .map_err(AppError::from)
        })
        .await
    }

    /// The tenant id owning the tool `name` that `org.<name>` names for
    /// `caller_id`: a tool of another live member of the caller's org with
    /// `visibility = 'org'`, else (so the shipped `share_revoked` window can
    /// answer) one that member has since unshared.
    pub async fn org_tool_owner(&self, caller_id: i64, name: String) -> Result<Option<i64>, AppError> {
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT t.tenant_id FROM tools t \
                 JOIN org_members mo ON mo.tenant_id = t.tenant_id AND mo.removed_unix IS NULL \
                 JOIN org_members mc ON mc.org_id = mo.org_id AND mc.tenant_id = ?1 \
                 AND mc.removed_unix IS NULL \
                 WHERE t.name = ?2 AND t.tenant_id != ?1 \
                 AND (t.visibility = 'org' OR (t.visibility = 'private' AND t.unshared_unix IS NOT NULL)) \
                 ORDER BY (t.visibility = 'org') DESC, t.tenant_id LIMIT 1",
                params![caller_id, name],
                |r| r.get(0),
            )
            .optional()
            .map_err(AppError::from)
        })
        .await
    }

    /// Every live `visibility = 'org'` tool of the other live members of
    /// `caller_id`'s org, as `(owner namespace, tool name)`.
    pub async fn org_tools_for_caller(&self, caller_id: i64) -> Result<Vec<(String, String)>, AppError> {
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT te.namespace, t.name FROM tools t \
                 JOIN org_members mo ON mo.tenant_id = t.tenant_id AND mo.removed_unix IS NULL \
                 JOIN org_members mc ON mc.org_id = mo.org_id AND mc.tenant_id = ?1 \
                 AND mc.removed_unix IS NULL \
                 JOIN tenants te ON te.id = t.tenant_id \
                 WHERE t.visibility = 'org' AND t.tenant_id != ?1 \
                 ORDER BY t.name, te.namespace",
            )?;
            let rows = stmt
                .query_map(params![caller_id], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// PRD-mcphost-org-shared-tools-namespace-v2 R7 (AC7): `visibility = 'org'`
    /// for `tenant_id`'s tool `name`, refused with `org_tool_name_taken
    /// {publisher}` when another live member of the same org already shares an
    /// org tool of that name. The collision check and the write run in one
    /// `BEGIN IMMEDIATE` transaction, so concurrent shares of one fresh name
    /// leave exactly one org tool. Returns `false` when `name` is not one of
    /// the tenant's tools.
    pub async fn share_org_tool(
        &self,
        tenant_id: i64,
        name: String,
        description: Option<String>,
        expose_spec: bool,
    ) -> Result<bool, AppError> {
        let shared_unix = now_unix();
        let spec_exposed_unix = expose_spec.then_some(shared_unix);
        self.with_conn(move |conn| {
            crate::txn::with_tx(conn, crate::txn::TxBehavior::Immediate, |conn| {
                let Some((org_id, ..)) = membership_sync(conn, tenant_id)? else {
                    return Err(org_required_error_for("host.tool_share with visibility 'org' needs an org"));
                };
                let publisher: Option<String> = conn
                    .query_row(
                        "SELECT te.namespace FROM tools t \
                         JOIN org_members m ON m.tenant_id = t.tenant_id AND m.org_id = ?1 \
                         AND m.removed_unix IS NULL \
                         JOIN tenants te ON te.id = t.tenant_id \
                         WHERE t.name = ?2 AND t.visibility = 'org' AND t.tenant_id != ?3 LIMIT 1",
                        params![org_id, name, tenant_id],
                        |r| r.get(0),
                    )
                    .optional()?;
                if let Some(publisher) = publisher {
                    return Err(AppError::Structured {
                        code: ORG_TOOL_NAME_TAKEN,
                        message: format!("org.{name} is already shared by {publisher}"),
                        data: json!({"publisher": publisher, "tool": name}),
                    });
                }
                let n = conn.execute(
                    "UPDATE tools SET visibility = 'org', shared_group = NULL, share_description = ?1, \
                     shared_unix = ?2, unshared_by = NULL, expose_spec = ?3, spec_exposed_unix = ?4, \
                     spec_reads = 0 WHERE tenant_id = ?5 AND name = ?6",
                    params![description, shared_unix, expose_spec as i64, spec_exposed_unix, tenant_id, name],
                )?;
                Ok(n > 0)
            })
        })
        .await
    }

    /// `host.org.remove`: end `member_ns`'s live membership in the org
    /// `owner_tenant_id` owns. Only `removed_unix` is written; the removed
    /// tenant keeps its key, tools and URL.
    pub async fn remove_org_member(
        &self,
        owner_tenant_id: i64,
        member_ns: String,
    ) -> Result<(), AppError> {
        let now = now_unix();
        self.with_conn(move |conn| {
            crate::txn::with_tx(conn, crate::txn::TxBehavior::Immediate, |conn| {
                let Some((org_id, _, role, _)) = membership_sync(conn, owner_tenant_id)? else {
                    return Err(not_owner_error());
                };
                if role != "owner" {
                    return Err(not_owner_error());
                }
                let target: Option<i64> = conn
                    .query_row(
                        "SELECT m.tenant_id FROM org_members m JOIN tenants t ON t.id = m.tenant_id \
                         WHERE m.org_id = ?1 AND t.namespace = ?2 AND m.removed_unix IS NULL",
                        params![org_id, member_ns],
                        |r| r.get(0),
                    )
                    .optional()?;
                let Some(target) = target else {
                    return Err(AppError::Structured {
                        code: NOT_A_MEMBER,
                        message: format!("'{member_ns}' is not a live member of this org"),
                        data: json!({"member": member_ns}),
                    });
                };
                if target == owner_tenant_id {
                    return Err(AppError::Structured {
                        code: OWNER_CANNOT_LEAVE,
                        message: "the owner cannot be removed from its own org".to_string(),
                        data: json!({}),
                    });
                }
                conn.execute(
                    "UPDATE org_members SET removed_unix = ?1 WHERE tenant_id = ?2",
                    params![now, target],
                )?;
                conn.execute(
                    "INSERT INTO tenant_audit (tenant_id, subject, action, detail, created_unix) \
                     VALUES (?1, ?2, 'org.remove', NULL, ?3)",
                    params![owner_tenant_id, member_ns, now],
                )?;
                Ok(())
            })
        })
        .await
    }

    /// `host.org.leave`: the caller ends its own live membership; the owner
    /// is refused. The audit row lands on the owner's tenant.
    pub async fn leave_org(&self, tenant_id: i64, namespace: String) -> Result<String, AppError> {
        let now = now_unix();
        self.with_conn(move |conn| {
            crate::txn::with_tx(conn, crate::txn::TxBehavior::Immediate, |conn| {
                let Some((org_id, name, role, _)) = membership_sync(conn, tenant_id)? else {
                    return Err(not_in_org_error());
                };
                if role == "owner" {
                    return Err(AppError::Structured {
                        code: OWNER_CANNOT_LEAVE,
                        message: "the owner cannot leave its own org".to_string(),
                        data: json!({"org": name}),
                    });
                }
                conn.execute(
                    "UPDATE org_members SET removed_unix = ?1 WHERE tenant_id = ?2",
                    params![now, tenant_id],
                )?;
                let owner_tenant_id: i64 = conn.query_row(
                    "SELECT owner_tenant_id FROM orgs WHERE id = ?1",
                    params![org_id],
                    |r| r.get(0),
                )?;
                conn.execute(
                    "INSERT INTO tenant_audit (tenant_id, subject, action, detail, created_unix) \
                     VALUES (?1, ?2, 'org.leave', NULL, ?3)",
                    params![owner_tenant_id, namespace, now],
                )?;
                Ok(name)
            })
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

pub(crate) fn not_in_org_error() -> AppError {
    AppError::Structured {
        code: NOT_IN_ORG,
        message: "this tenant does not belong to an org".to_string(),
        data: json!({}),
    }
}

pub(crate) fn not_owner_error() -> AppError {
    AppError::Structured {
        code: NOT_OWNER,
        message: "only the org owner can do this".to_string(),
        data: json!({}),
    }
}

/// The caller's org when it owns one: `(org_id, org name)`. `not_in_org` /
/// `not_owner` otherwise.
pub(crate) async fn owned_org(state: &AppState, tenant: &Tenant) -> Result<(i64, String), AppError> {
    let Some((org_id, name, role, _)) = state.db.org_membership(tenant.id).await? else {
        return Err(not_in_org_error());
    };
    if role != "owner" {
        return Err(not_owner_error());
    }
    Ok((org_id, name))
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

/// `org_required`: an org verb that reads org figures, called by a tenant in
/// no org; `data.next_call` is the verb that creates one.
pub(crate) fn org_required_error() -> AppError {
    org_required_error_for("host.org.usage reads an org's figures")
}

/// [`org_required_error`] with the reason clause naming the verb or call that
/// needed the org; the code and `data` are the same.
pub(crate) fn org_required_error_for(reason: &str) -> AppError {
    AppError::Structured {
        code: ORG_REQUIRED,
        message: format!("this tenant belongs to no org; {reason}"),
        data: json!({"next_call": "host.org.create", "hint": "Call host.org.create {name} to start an org, or host.org.join {code} to join one."}),
    }
}

/// `host.org.members {role?}`: every live member, for the owner and for
/// members alike.
pub async fn members(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let Some((org_id, name, role, _)) = state.db.org_membership(tenant.id).await? else {
        return Err(AppError::Structured {
            code: NOT_IN_ORG,
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

/// `host.org.remove {member}`: owner only; ends one membership.
pub async fn remove(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let member = arg_str(args, "member")?;
    state.db.remove_org_member(tenant.id, member.clone()).await?;
    // PRD-mcphost-org-shared-tools-namespace-v2 R5: the removed member's next
    // `tools/list` carries `ttlMs: 0`, so no client keeps `org.<tool>` cached.
    if let Some(gone) = state.db.find_tenant_by_namespace(member.clone()).await? {
        state.db.touch_tool_change_for(vec![gone.id]).await?;
    }
    Ok(json!({"removed": member}))
}

/// `host.org.leave`: a member ends its own membership.
pub async fn leave(state: &AppState, tenant: &Tenant, _args: &Value) -> Result<Value, AppError> {
    let org = state.db.leave_org(tenant.id, tenant.namespace.clone()).await?;
    state.db.touch_tool_change_for(vec![tenant.id]).await?;
    Ok(json!({"left": org}))
}

/// `host.quickstart`'s `org` block: the verb names, generated from [`TOOLS`].
pub fn quickstart_json() -> Value {
    json!({"tools": TOOLS, "roles": ROLES})
}

/// `host.whoami`'s `org` value: `{name, role, joined_at}` or null.
pub async fn whoami_org(state: &AppState, tenant_id: i64) -> Result<Value, AppError> {
    Ok(match state.db.org_membership(tenant_id).await? {
        Some((_, name, role, joined)) => json!({"name": name, "role": role, "joined_at": joined}),
        None => Value::Null,
    })
}

crate::err_code! {
    pub NOT_A_MEMBER,
    code: "not_a_member", family: Share, class: AgentFault, retryable: false,
    remedy: "Call host.org_members for the live member list; this namespace is not one of them.",
}

crate::err_code! {
    pub NOT_IN_ORG,
    code: "not_in_org", family: Share, class: AgentFault, retryable: false,
    remedy: "Call host.org_create or host.org_join first; this tenant belongs to no org.",
}

crate::err_code! {
    pub NOT_OWNER,
    code: "not_owner", family: Share, class: HostRefusal, retryable: false,
    remedy: "Only the org owner can do this; ask the owner listed by host.org_members.",
}

crate::err_code! {
    pub ORG_MEMBER_ALREADY,
    code: "org_member_already", family: Share, class: AgentFault, retryable: false,
    remedy: "This tenant is already in an org; call host.org_members to see it.",
}

crate::err_code! {
    pub ORG_MEMBERS_MAX,
    code: "org_members_max", family: Plan, class: PlanLimit, retryable: false,
    remedy: "Upgrade to the plan named in data.upgrade, or remove a member first.",
}

crate::err_code! {
    pub ORG_NAME_TAKEN,
    code: "org_name_taken", family: Share, class: AgentFault, retryable: false,
    remedy: "Choose a different org name; this one is already taken.",
}

crate::err_code! {
    pub OWNER_CANNOT_LEAVE,
    code: "owner_cannot_leave", family: Share, class: AgentFault, retryable: false,
    remedy: "The owner cannot be removed from its own org; remove other members or delete the org instead.",
}


crate::err_code! {
    pub ORG_TOOL_NAME_TAKEN,
    code: "org_tool_name_taken", family: Share, class: AgentFault, retryable: false,
    remedy: "Another org member already shares a tool under this name as org.<name>; rename yours or ask data.publisher to unshare theirs.",
}

crate::err_code! {
    pub ORG_REQUIRED,
    code: "org_required", family: Share, class: AgentFault, retryable: false,
    remedy: "Call host.org.create (or host.org.join) first; org usage needs an org.",
}
