//! The weekly org usage digest (PRD-mcphost-org-cost-attribution R3/R4): one
//! plain-text email per org to its verified owner, rendered from the same
//! `host.org.usage {window: "7d"}` rows, sent through [`crate::email`]. The
//! body carries counts and namespaces only -- never a key, a secret or a
//! tenant's stored data -- and ends with the opt-out line.

use rusqlite::params;
use serde_json::{Value, json};

use crate::db::{Db, Tenant};
use crate::email::EmailMessage;
use crate::errors::{AppError, RATE_LIMITED};
use crate::state::{AppState, now_unix};

/// Minimum seconds between two digests for one org (manual or weekly).
pub const SEND_INTERVAL_SECS: i64 = 3600;
/// The `tenant_audit` action a sent digest writes (also the rate-limit clock).
pub const AUDIT_SEND: &str = "org.digest.send";
/// The `tenant_audit` action `host.org.digest.set` writes.
pub const AUDIT_SET: &str = "org.digest.set";
/// The line every digest ends with.
pub const OPT_OUT_LINE: &str = "To stop these emails, call host.org.digest.set {enabled: false}.";

/// An org the weekly job considers.
pub struct DigestOrg {
    pub org_id: i64,
    pub name: String,
    pub owner_tenant_id: i64,
    pub digest_enabled: bool,
}

impl Db {
    /// Every org with its owner and opt-out flag, oldest first.
    pub async fn digest_orgs(&self) -> Result<Vec<DigestOrg>, AppError> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, name, owner_tenant_id, digest_enabled FROM orgs ORDER BY id",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    Ok(DigestOrg {
                        org_id: r.get(0)?,
                        name: r.get(1)?,
                        owner_tenant_id: r.get(2)?,
                        digest_enabled: r.get::<_, i64>(3)? != 0,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// Set the opt-out flag of the org `owner_tenant_id` owns; false when it owns none.
    pub async fn set_org_digest_enabled(&self, owner_tenant_id: i64, enabled: bool) -> Result<bool, AppError> {
        self.with_conn(move |conn| {
            let n = conn.execute(
                "UPDATE orgs SET digest_enabled = ?2 WHERE owner_tenant_id = ?1",
                params![owner_tenant_id, i64::from(enabled)],
            )?;
            Ok(n > 0)
        })
        .await
    }

    /// Unix time of the owner tenant's newest sent digest, if any.
    pub async fn last_digest_unix(&self, owner_tenant_id: i64) -> Result<Option<i64>, AppError> {
        self.with_conn(move |conn| {
            conn.query_row(
                "SELECT MAX(created_unix) FROM tenant_audit WHERE tenant_id = ?1 AND action = ?2",
                params![owner_tenant_id, AUDIT_SEND],
                |r| r.get(0),
            )
            .map_err(AppError::from)
        })
        .await
    }
}

/// ISO-8601 week number of the UTC day containing `unix`.
pub fn iso_week(unix: i64) -> i64 {
    let days = unix.div_euclid(86_400);
    let weekday = (days + 3).rem_euclid(7); // Monday = 0
    let thursday = days - weekday + 3;
    let year = civil_year(thursday);
    (thursday - days_from_civil(year, 1, 1)) / 7 + 1
}

fn civil_year(days: i64) -> i64 {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let year = yoe + era * 400;
    if mp >= 10 { year + 1 } else { year }
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// True during the minute the weekly job fires: Monday 07:00 UTC.
pub fn is_digest_minute(unix: i64) -> bool {
    let days = unix.div_euclid(86_400);
    (days + 3).rem_euclid(7) == 0 && unix.rem_euclid(86_400) / 60 == 7 * 60
}

fn money(v: &Value) -> String {
    v.as_f64().map_or_else(|| "-".to_string(), |c| format!("${c:.2}"))
}

/// The digest body: the two tables and the org totals as text.
pub fn render_body(org: &str, usage: &Value) -> String {
    let mut out = format!("{org} on mcphost, last 7 days\n\nMEMBERS (namespace, calls, errors, seconds, est cost)\n");
    for m in usage["members"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "- {}: {} calls, {} errors, {}s, {}\n",
            m["namespace"].as_str().unwrap_or("?"),
            m["calls"],
            m["errors"],
            m["seconds"],
            money(&m["est_cost_usd"]),
        ));
    }
    out.push_str("\nTOOLS (name, calls, callers)\n");
    for t in usage["tools"].as_array().into_iter().flatten() {
        out.push_str(&format!("- {}: {} calls, {} callers\n", t["name"].as_str().unwrap_or("?"), t["calls"], t["callers"]));
    }
    let t = &usage["totals"];
    out.push_str(&format!(
        "\nTOTAL: {} calls, {} errors, {}s, {}\n\n{OPT_OUT_LINE}\n",
        t["calls"],
        t["errors"],
        t["seconds"],
        money(&t["est_cost_usd"]),
    ));
    out
}

fn rate_limited(wait_s: i64) -> AppError {
    AppError::Structured {
        code: RATE_LIMITED,
        message: "this org's digest was sent less than an hour ago".to_string(),
        data: json!({"retry_after_s": wait_s}),
    }
}

/// Send `owner`'s org digest now. `rate_limited` when one went out in the
/// last [`SEND_INTERVAL_SECS`]. The caller has already checked ownership and
/// that the owner is verified with an address.
pub async fn send_digest(state: &AppState, owner: &Tenant, now: i64) -> Result<Value, AppError> {
    let Some(to) = owner.owner_email.clone() else {
        return Err(AppError::InvalidArgs("the owner has no verified email address".to_string()));
    };
    if let Some(last) = state.db.last_digest_unix(owner.id).await?
        && now - last < SEND_INTERVAL_SECS
    {
        return Err(rate_limited(SEND_INTERVAL_SECS - (now - last)));
    }
    let usage = crate::org_usage::usage(state, owner, &json!({"window": "7d"})).await?;
    let org = usage["org"].as_str().unwrap_or_default().to_string();
    let from = state.email_config.from.clone().unwrap_or_default();
    let message = EmailMessage {
        to,
        reply_to: crate::email::reply_to_address(&from),
        from,
        subject: format!("{org} on mcphost, week {}", iso_week(now)),
        text_body: render_body(&org, &usage),
        ..EmailMessage::default()
    };
    state.email_client.send(&message).await?;
    state.db.record_tenant_audit(owner.id, "owner".to_string(), AUDIT_SEND.to_string(), Some(org.clone())).await?;
    Ok(json!({"sent": true, "org": org}))
}

/// One pass of the weekly job: for each org, send or skip, and return one
/// log line per org -- `org=<name> sent` or `org=<name> skipped:<reason>`.
/// Lines (and the matching `tracing` events) never carry an address or key.
pub async fn weekly_tick(state: &AppState, now: i64) -> Result<Vec<String>, AppError> {
    let mut lines = Vec::new();
    for org in state.db.digest_orgs().await? {
        let owner = state.db.find_tenant_by_id(org.owner_tenant_id).await?;
        let outcome = match owner {
            Some(o) if o.owner_verified_at.is_some() && o.owner_email.is_some() => {
                if !org.digest_enabled {
                    "skipped:disabled".to_string()
                } else {
                    match send_digest(state, &o, now).await {
                        Ok(_) => "sent".to_string(),
                        Err(AppError::Structured { code, .. }) if code == RATE_LIMITED => "skipped:rate_limited".to_string(),
                        Err(e) => {
                            tracing::warn!(error = %e, org = %org.name, "org digest send failed");
                            "skipped:send_failed".to_string()
                        }
                    }
                }
            }
            _ => "skipped:no_verified_owner".to_string(),
        };
        let line = format!("org={} {outcome}", org.name);
        tracing::info!("{line}");
        lines.push(line);
    }
    Ok(lines)
}

/// Started once from `main.rs`: checks every 30 s and runs [`weekly_tick`]
/// in the Monday 07:00 UTC minute (the 1 h send interval absorbs a second
/// check inside that minute).
pub fn spawn_weekly(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            let now = now_unix();
            if is_digest_minute(now)
                && let Err(e) = weekly_tick(&state, now).await
            {
                tracing::warn!(error = %e, "org digest tick failed");
            }
        }
    })
}

/// The caller's org when it owns one: `(org_id, org name)`. `not_in_org` /
/// `not_owner` otherwise.
async fn owned_org(state: &AppState, tenant: &Tenant) -> Result<(i64, String), AppError> {
    let Some((org_id, name, role, _)) = state.db.org_membership(tenant.id).await? else {
        return Err(crate::org::not_in_org_error());
    };
    if role != "owner" {
        return Err(crate::org::not_owner_error());
    }
    Ok((org_id, name))
}

/// `host.org.digest.set {enabled}`: the owner opts the weekly digest in or out.
pub async fn set(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let (_, org) = owned_org(state, tenant).await?;
    let enabled = args
        .get("enabled")
        .and_then(Value::as_bool)
        .ok_or_else(|| AppError::InvalidArgs("missing required boolean argument 'enabled'".to_string()))?;
    state.db.set_org_digest_enabled(tenant.id, enabled).await?;
    state.db.record_tenant_audit(tenant.id, "owner".to_string(), AUDIT_SET.to_string(), Some(format!("{org} enabled={enabled}"))).await?;
    Ok(json!({"org": org, "digest_enabled": enabled}))
}

/// `host.org.digest.send`: the owner sends the 7d digest now (1 per hour).
pub async fn send(state: &AppState, tenant: &Tenant, _args: &Value) -> Result<Value, AppError> {
    owned_org(state, tenant).await?;
    // Read fresh: a claim completing between two calls must be seen.
    let fresh = state
        .db
        .find_tenant_by_id(tenant.id)
        .await?
        .ok_or_else(|| AppError::Internal("tenant vanished mid-call".to_string()))?;
    if fresh.owner_verified_at.is_none() || fresh.owner_email.is_none() {
        return Err(AppError::Structured {
            code: crate::notify::OWNER_UNVERIFIED,
            message: "the digest goes to the owner's verified address; this tenant's owner has not verified one".to_string(),
            data: json!({"next_call": "host.claim"}),
        });
    }
    send_digest(state, &fresh, now_unix()).await
}
