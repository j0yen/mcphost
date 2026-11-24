//! PRD-mcphost-org-export-and-offboard-v2 R4/R5: `host.org.offboard` and
//! `host.org.restore` -- the owner switches the whole team off with one
//! confirmed call, and back on at any age. Nothing is deleted: an
//! org-offboarded tenant keeps its tools and state and stays disabled with
//! `disabled_reason = org_offboard` until restored.

use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};

use crate::auth_shape::ORG_OFFBOARD_REASON;
use crate::db::{Db, Tenant};
use crate::errors::AppError;
use crate::state::{AppState, now_unix};

/// `tenant_audit.action` of one member switched off by `host.org.offboard`.
pub const AUDIT_OFFBOARD: &str = "org.offboard";
/// `tenant_audit.action` of one member switched back on by `host.org.restore`.
pub const AUDIT_RESTORE: &str = "org.restore";

/// A member `host.org.offboard` disabled: its tenant id (for the teardown).
pub struct Flipped {
    pub tenant_id: i64,
}

/// One member's tenant id `host.org.restore` re-enabled.
pub type Restored = i64;

impl Db {
    /// When the org's owner offboarded it; `None` while it is switched on.
    pub async fn org_offboarded_unix(&self, org_id: i64) -> Result<Option<i64>, AppError> {
        self.with_conn(move |conn| {
            conn.query_row("SELECT offboarded_unix FROM orgs WHERE id = ?1", params![org_id], |r| r.get(0))
                .optional()
                .map(Option::flatten)
                .map_err(AppError::from)
        })
        .await
    }

    /// The run id of an org export `owner_tenant_id` has running, if any.
    pub async fn running_org_export(&self, owner_tenant_id: i64) -> Result<Option<String>, AppError> {
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT id FROM runs WHERE tenant_id = ?1 AND tool_name = ?2 AND status = 'running' \
                 ORDER BY rowid LIMIT 1",
                params![owner_tenant_id, crate::export::ORG_EXPORT_TOOL_NAME],
                |r| r.get(0),
            )
            .optional()
            .map_err(AppError::from)
        })
        .await
    }

    /// In one transaction: disables every live member of `org_id` that is not
    /// already disabled (`disabled_reason = org_offboard`), writes one
    /// `org.offboard` audit row per member it flipped, and stamps
    /// `orgs.offboarded_unix`. Returns the members it flipped.
    pub async fn offboard_org(&self, org_id: i64, org_name: String, now: i64) -> Result<Vec<Flipped>, AppError> {
        self.with_conn(move |conn| {
            crate::txn::with_tx(conn, crate::txn::TxBehavior::Immediate, |conn| {
                let live: Vec<i64> = conn
                    .prepare("SELECT tenant_id FROM org_members WHERE org_id = ?1 AND removed_unix IS NULL ORDER BY joined_unix, tenant_id")?
                    .query_map(params![org_id], |r| r.get(0))?
                    .collect::<Result<_, _>>()?;
                let mut flipped = Vec::new();
                for tenant_id in live {
                    let n = conn.execute(
                        "UPDATE tenants SET disabled = 1, disabled_reason = ?2 WHERE id = ?1 AND disabled = 0",
                        params![tenant_id, ORG_OFFBOARD_REASON],
                    )?;
                    if n > 0 {
                        conn.execute(
                            "INSERT INTO tenant_audit (tenant_id, subject, action, detail, created_unix) \
                             VALUES (?1, 'owner', ?2, ?3, ?4)",
                            params![tenant_id, AUDIT_OFFBOARD, org_name, now],
                        )?;
                        flipped.push(Flipped { tenant_id });
                    }
                }
                conn.execute("UPDATE orgs SET offboarded_unix = ?2 WHERE id = ?1", params![org_id, now])?;
                Ok(flipped)
            })
        })
        .await
    }

    /// In one transaction: re-enables every live member of `org_id` whose
    /// `disabled_reason` is `org_offboard` (reason back to NULL, a fresh idle
    /// run so no expiry sweep sees the offboarded years as idleness), writes
    /// one `org.restore` audit row per member, and clears
    /// `orgs.offboarded_unix`. A member disabled for any other reason keeps
    /// its state. Nothing is deleted or re-created.
    pub async fn restore_org(&self, org_id: i64, org_name: String, now: i64) -> Result<Vec<Restored>, AppError> {
        self.with_conn(move |conn| {
            crate::txn::with_tx(conn, crate::txn::TxBehavior::Immediate, |conn| {
                let due: Vec<i64> = conn
                    .prepare(
                        "SELECT m.tenant_id FROM org_members m JOIN tenants t ON t.id = m.tenant_id \
                         WHERE m.org_id = ?1 AND m.removed_unix IS NULL AND t.disabled = 1 \
                         AND t.disabled_reason = ?2 ORDER BY m.joined_unix, m.tenant_id",
                    )?
                    .query_map(params![org_id, ORG_OFFBOARD_REASON], |r| r.get(0))?
                    .collect::<Result<_, _>>()?;
                for tenant_id in &due {
                    conn.execute(
                        "UPDATE tenants SET disabled = 0, disabled_reason = NULL, last_seen_unix = ?2 WHERE id = ?1",
                        params![tenant_id, now],
                    )?;
                    conn.execute(
                        "INSERT INTO tenant_audit (tenant_id, subject, action, detail, created_unix) \
                         VALUES (?1, 'owner', ?2, ?3, ?4)",
                        params![tenant_id, AUDIT_RESTORE, org_name, now],
                    )?;
                }
                conn.execute("UPDATE orgs SET offboarded_unix = NULL WHERE id = ?1", params![org_id])?;
                Ok(due)
            })
        })
        .await
    }
}

/// `host.org.offboard {confirm}`: owner only. Same order as `self_offboard`:
/// billing first (a failure aborts before any flip, and a retry is safe), then
/// the flip, then the teardown of what each member had running.
pub async fn offboard(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let (org_id, org_name) = crate::org::owned_org(state, tenant).await?;
    // A repeat call by the already-offboarded owner reaches the verb through
    // `auth_shape::lets_through_disabled` and answers idempotently.
    if tenant.disabled && tenant.disabled_reason.as_deref() == Some(ORG_OFFBOARD_REASON) {
        let at = state.db.org_offboarded_unix(org_id).await?;
        return Ok(json!({"org": org_name, "offboarded_unix": at.unwrap_or(0), "already_offboarded": true}));
    }
    let confirm = args
        .get("confirm")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::InvalidArgs("missing required argument 'confirm'".to_string()))?;
    if confirm != org_name {
        return Err(AppError::Structured {
            code: CONFIRM_MISMATCH,
            message: "confirm must equal the org's name; nothing was changed".to_string(),
            data: json!({}),
        });
    }
    // The owner's key goes dark with this call, so a running export could no
    // longer be polled for its download_url.
    if let Some(run_id) = state.db.running_org_export(tenant.id).await? {
        return Err(AppError::Structured {
            code: ORG_EXPORT_RUNNING,
            message: "an org export is running; wait for it to finish, then offboard".to_string(),
            data: json!({"run_id": run_id}),
        });
    }

    let mut billing_canceled: Vec<String> = Vec::new();
    for member in state.db.org_live_members(org_id).await? {
        let Some(row) = state.db.find_tenant_by_id(member.tenant_id).await? else { continue };
        if row.disabled {
            continue;
        }
        if row.plan == "pro"
            && let Some(customer) = row.stripe_customer_id.as_deref()
        {
            billing_canceled.extend(state.billing_client.cancel_active_subscriptions(customer).await?);
        }
    }

    let now = now_unix();
    let flipped = state.db.offboard_org(org_id, org_name.clone(), now).await?;
    for member in &flipped {
        crate::control::release_tenant_runtime(state, member.tenant_id).await;
    }
    tracing::info!(org = %org_name, members = flipped.len(), "org offboarded");
    Ok(json!({
        "org": org_name,
        "members_disabled": flipped.len(),
        "offboarded_unix": now,
        "billing_canceled": billing_canceled,
    }))
}

/// `host.org.restore`: owner only. The owner's key is let through the
/// disabled-tenant refusal for this verb (`auth_shape::DISABLED_PASSES`); a
/// non-owner is refused `not_owner` before anything is read or written. There
/// is no restore window: it succeeds at any age, and re-enabling is all it
/// does -- no subscription is re-created, a member returns on the plan its
/// row then holds.
pub async fn restore(state: &AppState, tenant: &Tenant, _args: &Value) -> Result<Value, AppError> {
    let (org_id, org_name) = crate::org::owned_org(state, tenant).await?;
    let restored = state.db.restore_org(org_id, org_name.clone(), now_unix()).await?;
    tracing::info!(org = %org_name, members = restored.len(), "org restored");
    Ok(json!({"org": org_name, "members_restored": restored.len()}))
}

crate::err_code! {
    pub CONFIRM_MISMATCH,
    code: "confirm_mismatch", family: Share, class: AgentFault, retryable: false,
    remedy: "Pass the org's exact name as confirm (host.org.members names it); nothing was changed.",
}

crate::err_code! {
    pub ORG_EXPORT_RUNNING,
    code: "org_export_running", family: Share, class: AgentFault, retryable: false,
    remedy: "An org export is running; poll host.runs.get with data.run_id until it finishes, then offboard.",
}
