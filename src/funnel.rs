//! `mcphost funnel` (PRD-mcphost-tenant-attribution P0 requirement 4, AC5):
//! signups -> published a tool -> first successful call -> returned on a
//! later day -> hit the daily cap -> upgraded, split real (`source_class =
//! external`) from synthetic (everything else), computed entirely from the
//! `tenants`/`tools`/`calls` tables this crate already has (technical
//! considerations: "reads the existing tables only", no new table). An
//! admin CLI report on the box, not a tenant-facing MCP tool -- see
//! `main.rs`'s `Funnel` subcommand.

use std::collections::{HashMap, HashSet};

use serde::Serialize;
use serde_json::{Value, json};

use crate::db::Db;
use crate::errors::AppError;
use crate::plans::PlanCatalog;

#[derive(Debug, Clone, Default, Serialize)]
pub struct FunnelStage {
    pub signed_up: i64,
    pub published: i64,
    pub called: i64,
    pub returned: i64,
    pub hit_cap: i64,
    pub upgraded: i64,
    /// Median seconds from signup to first successful call, over tenants
    /// with at least one; `None` when none have (AC5 doesn't say what to
    /// report for an empty class, and `0.0` would read as "instant").
    pub median_signup_to_first_call_secs: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct FunnelReport {
    pub real: FunnelStage,
    pub synthetic: FunnelStage,
}

impl FunnelReport {
    pub fn to_json(&self) -> Value {
        json!({"real": self.real, "synthetic": self.synthetic})
    }
}

fn median(values: &mut [i64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    let n = values.len();
    let mid = n / 2;
    if n.is_multiple_of(2) {
        Some((values[mid - 1] + values[mid]) as f64 / 2.0)
    } else {
        Some(values[mid] as f64)
    }
}

/// AC5: `mcphost funnel --json` in under 2s on the current DB. Three plain
/// table reads (never a join, never a per-tenant query in a loop) plus
/// in-process aggregation -- the small dataset this crate's SQLite file
/// holds today makes even a naive approach fast; this shape also stays
/// fast as `calls` grows, since it's still exactly one read of each table.
pub async fn compute(
    db: &Db,
    plans: &PlanCatalog,
    since_unix: Option<i64>,
) -> Result<FunnelReport, AppError> {
    let tenants = db.funnel_tenants(since_unix).await?;
    let published: HashSet<i64> = db.published_tenant_ids().await?;
    let calls = db.funnel_calls().await?;

    let in_scope: HashSet<i64> = tenants.iter().map(|(id, ..)| *id).collect();
    let mut by_tenant: HashMap<i64, Vec<(i64, bool)>> = HashMap::new();
    for (tenant_id, started_unix, ok) in calls {
        if in_scope.contains(&tenant_id) {
            by_tenant
                .entry(tenant_id)
                .or_default()
                .push((started_unix, ok));
        }
    }

    let mut real = FunnelStage::default();
    let mut synthetic = FunnelStage::default();
    let mut real_latencies: Vec<i64> = Vec::new();
    let mut synthetic_latencies: Vec<i64> = Vec::new();

    for (id, source_class, plan, created_unix) in &tenants {
        // requirement 3's same rule: "real" is exactly `external`.
        let is_real = source_class.as_deref() == Some("external");
        let stage = if is_real { &mut real } else { &mut synthetic };
        stage.signed_up += 1;

        if published.contains(id) {
            stage.published += 1;
        }

        let signup_day = created_unix.map(|u| u.div_euclid(86_400));
        let mut first_ok_unix: Option<i64> = None;
        let mut ok_days: HashSet<i64> = HashSet::new();
        let mut ok_count_by_day: HashMap<i64, i64> = HashMap::new();
        if let Some(tenant_calls) = by_tenant.get(id) {
            for (started_unix, ok) in tenant_calls {
                if *ok {
                    if first_ok_unix.is_none() {
                        first_ok_unix = Some(*started_unix);
                    }
                    let day = started_unix.div_euclid(86_400);
                    ok_days.insert(day);
                    *ok_count_by_day.entry(day).or_insert(0) += 1;
                }
            }
        }

        if let Some(first_ok) = first_ok_unix {
            stage.called += 1;
            if let Some(cu) = created_unix {
                let latency = (first_ok - cu).max(0);
                if is_real {
                    real_latencies.push(latency);
                } else {
                    synthetic_latencies.push(latency);
                }
            }
        }

        // "returned on a later day": an ok call on a UTC day strictly
        // after the signup day. `signup_day: None` (a row `created_unix`
        // couldn't be derived for) falls back to "more than one distinct
        // ok-call day" rather than dropping the tenant from this stage
        // entirely.
        let returned = match signup_day {
            Some(sd) => ok_days.iter().any(|d| *d > sd),
            None => ok_days.len() > 1,
        };
        if returned {
            stage.returned += 1;
        }

        // "hit the daily cap": a rejected call writes no `calls` row
        // (`handler::check_calls_quota` refuses before dispatch), so the
        // observable signal is a day whose *successful* count reached the
        // tenant's current plan's `calls_per_day` -- the last call that
        // day to actually succeed. Uses the tenant's plan *today*, not
        // whatever plan was active on that historical day (this table
        // doesn't record that); a defensible approximation, not an exact
        // historical replay.
        if let Some(cap) = plans.plans.iter().find(|p| &p.name == plan).map(|p| p.calls_per_day)
            && ok_count_by_day.values().any(|&c| c >= cap)
        {
            stage.hit_cap += 1;
        }

        if plan != "free" {
            stage.upgraded += 1;
        }
    }

    real.median_signup_to_first_call_secs = median(&mut real_latencies);
    synthetic.median_signup_to_first_call_secs = median(&mut synthetic_latencies);

    Ok(FunnelReport { real, synthetic })
}

/// PRD-mcphost-funnel-by-surface-nightly-probe R2: one install surface's
/// launch stages over humans only.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SurfaceStage {
    pub signed_up: i64,
    pub claimed: i64,
    pub published: i64,
    pub first_external_call: i64,
    /// Tenants with a successful call on days 7-14 after signup, counted
    /// over tenants at least 14 days old only; `None` (JSON `null`) while
    /// the row has no such tenant -- not yet measurable, never `0` (R6).
    pub day7_active: Option<i64>,
}

const DAY: i64 = 86_400;
/// Age at which `day7_active` becomes measurable (the end of the day 7-14
/// window).
const DAY7_MATURITY_SECS: i64 = 14 * DAY;
/// The `probe` line looks back this far.
const PROBE_WINDOW_SECS: i64 = DAY;

/// R3: `(by_surface, probe)` for the trailing `days` (default window is
/// whole UTC days, so a tenant born exactly `days` ago is inside it).
/// `by_surface` counts `funnel_origin = human` tenants under every
/// [`crate::install_links::SURFACES`] id plus `other`; `probe` lists the
/// `fleet`/`probe` tenants born in the last 24 h with the furthest stage
/// each reached.
pub async fn surface_report(db: &Db, now: i64, days: i64) -> Result<(Value, Value), AppError> {
    let since = (now - days * DAY).div_euclid(DAY) * DAY;
    let tenants = db.surface_funnel_tenants(since).await?;
    let mut calls: HashMap<i64, Vec<(i64, bool)>> = HashMap::new();
    for (tenant_id, started_unix, external) in db.surface_funnel_calls(since).await? {
        calls.entry(tenant_id).or_default().push((started_unix, external));
    }

    let mut rows: std::collections::BTreeMap<&'static str, SurfaceStage> = crate::install_links::SURFACES
        .iter()
        .copied()
        .chain(["other"])
        .map(|id| (id, SurfaceStage::default()))
        .collect();
    let mut probe: Vec<(i64, Value)> = Vec::new();

    for t in &tenants {
        let tenant_calls = calls.get(&t.id).map(Vec::as_slice).unwrap_or(&[]);
        let first_external = tenant_calls.iter().find(|(at, ext)| *ext && *at >= t.created_unix).map(|(at, _)| *at);
        let published = t.has_tool || t.first_publish_unix.is_some();
        match t.funnel_origin.as_str() {
            "human" => {
                let row = rows.entry(crate::install_links::surface_id(t.client_name.as_deref())).or_default();
                row.signed_up += 1;
                row.claimed += i64::from(t.claimed_unix.is_some());
                row.published += i64::from(published);
                row.first_external_call += i64::from(first_external.is_some());
                if now >= t.created_unix + DAY7_MATURITY_SECS {
                    let active = tenant_calls.iter().any(|(at, _)| {
                        *at >= t.created_unix + 7 * DAY && *at <= t.created_unix + DAY7_MATURITY_SECS
                    });
                    *row.day7_active.get_or_insert(0) += i64::from(active);
                }
            }
            "fleet" | "probe" if t.created_unix >= now - PROBE_WINDOW_SECS => {
                let (stage, reached_at) = if let Some(at) = first_external {
                    ("first_external_call", at)
                } else if published {
                    ("published", t.first_publish_unix.unwrap_or(t.created_unix))
                } else if let Some(at) = t.claimed_unix {
                    ("claimed", at)
                } else {
                    ("signed_up", t.created_unix)
                };
                probe.push((
                    t.created_unix,
                    json!({
                        "tenant_id": t.id,
                        "namespace": t.namespace,
                        "client_name": t.client_name,
                        "stage": stage,
                        "reached_at": reached_at,
                    }),
                ));
            }
            _ => {}
        }
    }
    probe.sort_by_key(|(created, _)| std::cmp::Reverse(*created));
    Ok((json!(rows), Value::Array(probe.into_iter().map(|(_, v)| v).collect())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_of_values() {
        assert_eq!(median(&mut []), None);
        assert_eq!(median(&mut [5]), Some(5.0));
        assert_eq!(median(&mut [1, 3]), Some(2.0));
        assert_eq!(median(&mut [3, 1, 2]), Some(2.0));
        assert_eq!(median(&mut [1, 2, 3, 4]), Some(2.5));
    }
}
