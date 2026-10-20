//! `host.org.usage` (PRD-mcphost-org-cost-attribution): per-member and
//! per-tool figures for a whole org, computed by the same [`Db::usage`] /
//! `jobs_usage` / `scheduled_usage` / `state_bytes_used` calls `host.usage`
//! makes for one tenant, so a member row cannot differ from that member's
//! own `host.usage` -- there is no second meter.

use rusqlite::params;
use serde_json::{Value, json};

use crate::db::{Db, Tenant};
use crate::errors::AppError;
use crate::state::{AppState, parse_window_secs};

/// Longest window `host.org.usage` reads (seconds).
pub const WINDOW_MAX_SECS: i64 = 90 * 86_400;
/// Window used when the caller names none.
pub const WINDOW_DEFAULT: &str = "7d";

/// One org seat, live or removed, with the plan its figures are priced at.
pub struct RosterRow {
    pub tenant_id: i64,
    pub namespace: String,
    pub plan: String,
    pub role: String,
    /// When the seat ended; `None` for a live member.
    pub removed_unix: Option<i64>,
}

impl Db {
    /// Every seat of `org_id` that was live at some point in the last
    /// `window_secs` (removed members included), owner first then by join time.
    pub async fn org_usage_roster(&self, org_id: i64, window_secs: i64) -> Result<Vec<RosterRow>, AppError> {
        let since = crate::state::now_unix() - window_secs;
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT t.id, t.namespace, t.plan, m.role, m.removed_unix FROM org_members m \
                 JOIN tenants t ON t.id = m.tenant_id WHERE m.org_id = ?1 \
                 AND (m.removed_unix IS NULL OR m.removed_unix >= ?2) ORDER BY m.joined_unix, t.id",
            )?;
            let rows = stmt
                .query_map(params![org_id, since], |r| {
                    Ok(RosterRow {
                        tenant_id: r.get(0)?,
                        namespace: r.get(1)?,
                        plan: r.get(2)?,
                        role: r.get(3)?,
                        removed_unix: r.get(4)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }
}

impl Db {
    /// One row per tool published by a live member of `org_id`:
    /// `(namespace, tool, calls, callers)` over the last `window_secs`.
    /// `callers` counts distinct calling tenants (the publisher itself when
    /// no `caller_tenant_id` is set). Refused and test calls are excluded,
    /// as in [`Db::usage`].
    pub async fn org_tool_usage(
        &self,
        org_id: i64,
        window_secs: i64,
    ) -> Result<Vec<(String, String, i64, i64)>, AppError> {
        let since = crate::state::now_unix() - window_secs;
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT t.namespace, x.name, \
                   (SELECT COUNT(*) FROM calls c WHERE c.tenant_id = x.tenant_id \
                      AND c.tool_name = x.name AND c.started_unix >= ?2 \
                      AND c.outcome IS NOT 'test' AND COALESCE(c.error_class, '') <> 'refused'), \
                   (SELECT COUNT(DISTINCT COALESCE(c.caller_tenant_id, c.tenant_id)) FROM calls c \
                      WHERE c.tenant_id = x.tenant_id AND c.tool_name = x.name \
                      AND c.started_unix >= ?2 AND c.outcome IS NOT 'test' \
                      AND COALESCE(c.error_class, '') <> 'refused') \
                 FROM org_members m JOIN tenants t ON t.id = m.tenant_id \
                 JOIN tools x ON x.tenant_id = t.id \
                 WHERE m.org_id = ?1 AND m.removed_unix IS NULL",
            )?;
            let rows = stmt
                .query_map(params![org_id, since], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }
}

/// One member's figures for the window. Every number comes from the call
/// `host.usage` itself makes.
async fn member_row(state: &AppState, seat: &RosterRow, secs: i64) -> Result<Value, AppError> {
    let stats = state.db.usage_until(seat.tenant_id, secs, seat.removed_unix).await?;
    // A removed member's calls are counted up to its removal; its seconds and
    // stored bytes are no longer the org's, so they are not attributed.
    let (seconds, state_bytes) = if seat.removed_unix.is_some() {
        (0, 0)
    } else {
        let jobs = state.db.jobs_usage(seat.tenant_id, secs).await?;
        let scheduled = state.db.scheduled_usage(seat.tenant_id, secs).await?;
        (jobs.seconds + scheduled.seconds, state.db.state_bytes_used(seat.tenant_id).await?)
    };
    let est = state.plans.get(&seat.plan).and_then(|p| p.est_cost_usd(stats.calls));
    Ok(json!({
        "namespace": seat.namespace,
        "role": seat.role,
        "removed": seat.removed_unix.is_some(),
        "calls": stats.calls,
        "errors": stats.errors,
        "seconds": seconds,
        "state_bytes": state_bytes,
        "est_cost_usd": est,
    }))
}

/// `host.org.usage {window?}`: the owner reads every member; see
/// [`crate::org`] for membership.
pub async fn usage(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let Some((org_id, name, role, _)) = state.db.org_membership(tenant.id).await? else {
        return Err(crate::org::org_required_error());
    };
    let window = args.get("window").and_then(Value::as_str).unwrap_or(WINDOW_DEFAULT).to_string();
    let secs = parse_window_secs(&window).min(WINDOW_MAX_SECS);
    let mut members = Vec::new();
    for seat in state.db.org_usage_roster(org_id, secs).await? {
        members.push(member_row(state, &seat, secs).await?);
    }
    members.sort_by_key(|m| std::cmp::Reverse(m["calls"].as_i64().unwrap_or(0)));
    // Totals cover the whole org; a member is shown only its own row.
    let sum = |key: &str| members.iter().map(|m| m[key].as_i64().unwrap_or(0)).sum::<i64>();
    let est_total: Option<f64> = {
        let priced: Vec<f64> = members.iter().filter_map(|m| m["est_cost_usd"].as_f64()).collect();
        (!priced.is_empty()).then(|| priced.iter().sum())
    };
    let totals = json!({
        "calls": sum("calls"),
        "errors": sum("errors"),
        "seconds": sum("seconds"),
        "state_bytes": sum("state_bytes"),
        "est_cost_usd": est_total,
    });
    if role != "owner" {
        members.retain(|m| m["namespace"] == json!(tenant.namespace));
        return Ok(json!({"org": name, "window": window, "members": members, "totals": totals}));
    }
    let mut tools: Vec<Value> = state
        .db
        .org_tool_usage(org_id, secs)
        .await?
        .into_iter()
        .map(|(publisher, tool, calls, callers)| {
            json!({"name": format!("{publisher}.{tool}"), "publisher": publisher, "calls": calls, "callers": callers})
        })
        .collect();
    tools.sort_by_key(|t| std::cmp::Reverse(t["calls"].as_i64().unwrap_or(0)));
    Ok(json!({"org": name, "window": window, "members": members, "tools": tools, "totals": totals}))
}
