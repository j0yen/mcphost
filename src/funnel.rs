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
#[derive(Debug, Clone, Default)]
pub struct SurfaceStage {
    pub signed_up: i64,
    pub claimed: i64,
    pub published: i64,
    /// Tenants with a successful call as themselves at or after signup
    /// (`caller_tenant_id` NULL or the tenant, no end user): the exact
    /// complement of `first_external_call`, so every successful call lands
    /// in exactly one of the two cells.
    pub first_own_call: i64,
    pub first_external_call: i64,
    /// Tenants with a successful call on days 7-14 after signup, counted
    /// over tenants at least 14 days old only; `None` (JSON `null`) while
    /// the row has no such tenant -- not yet measurable, never `0` (R6).
    pub day7_active: Option<i64>,
}

/// The row serializes through [`crate::api_contract::SURFACE_STAGE_KEYS`], so
/// its keys are the const and a key without a value here panics in the
/// drift test rather than shipping.
impl Serialize for SurfaceStage {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(crate::api_contract::SURFACE_STAGE_KEYS.len()))?;
        for key in crate::api_contract::SURFACE_STAGE_KEYS {
            match *key {
                "signed_up" => map.serialize_entry(key, &self.signed_up)?,
                "claimed" => map.serialize_entry(key, &self.claimed)?,
                "published" => map.serialize_entry(key, &self.published)?,
                "first_own_call" => map.serialize_entry(key, &self.first_own_call)?,
                "first_external_call" => map.serialize_entry(key, &self.first_external_call)?,
                "day7_active" => map.serialize_entry(key, &self.day7_active)?,
                other => unreachable!("SURFACE_STAGE_KEYS names {other}, which SurfaceStage does not carry"),
            }
        }
        map.end()
    }
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
/// each reached. The third element is `synthetic_excluded`: the tenants that
/// would have been `human` rows but are known synthetic
/// ([`crate::state::is_synthetic_tenant`]) and so sit in no surface row.
pub async fn surface_report(db: &Db, now: i64, days: i64) -> Result<(Value, Value, i64), AppError> {
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
    let mut synthetic_excluded = 0i64;

    for t in &tenants {
        let tenant_calls = calls.get(&t.id).map(Vec::as_slice).unwrap_or(&[]);
        let first_external = tenant_calls.iter().find(|(at, ext)| *ext && *at >= t.created_unix).map(|(at, _)| *at);
        let first_own = tenant_calls.iter().any(|(at, ext)| !*ext && *at >= t.created_unix);
        let published = t.has_tool || t.first_publish_unix.is_some();
        match t.funnel_origin.as_str() {
            "human" => {
                if crate::state::is_synthetic_tenant(
                    t.client_name.as_deref(),
                    t.source_class.as_deref(),
                    t.origin.as_deref(),
                    &t.funnel_origin,
                ) {
                    synthetic_excluded += 1;
                    continue;
                }
                let row = rows.entry(crate::install_links::surface_id(t.client_name.as_deref())).or_default();
                row.signed_up += 1;
                row.claimed += i64::from(t.claimed_unix.is_some());
                row.published += i64::from(published);
                row.first_own_call += i64::from(first_own);
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
    Ok((json!(rows), Value::Array(probe.into_iter().map(|(_, v)| v).collect()), synthetic_excluded))
}

/// `(recipe_attempts_7d, recipe_attempts_synthetic_7d)`: `{recipe: count}`
/// with every [`crate::kinds::aliases::recipe_names`] name present
/// (zero-filled), and one integer for the synthetic tenants' uses. Counts
/// and recipe names only -- no tenant id, namespace or client name.
pub async fn recipe_attempts_7d(db: &Db, now: i64) -> Result<(Value, i64), AppError> {
    let mut attempts: std::collections::BTreeMap<String, i64> =
        crate::kinds::aliases::recipe_names().into_iter().map(|name| (name.to_string(), 0)).collect();
    let mut synthetic = 0i64;
    for row in db.recipe_uses_since(now - 7 * DAY).await? {
        if crate::state::is_synthetic_tenant(
            row.client_name.as_deref(),
            row.source_class.as_deref(),
            row.origin.as_deref(),
            &row.funnel_origin,
        ) {
            synthetic += 1;
        } else if let Some(count) = attempts.get_mut(&row.recipe) {
            *count += 1;
        }
    }
    Ok((json!(attempts), synthetic))
}

// ---- activation milestones (PRD-mcphost-time-to-first-success-public) ----

/// R1: the activation milestones, in order, each with the tool that reaches
/// it (`""` when none does -- `second_session` is time, not a call).
/// `admin.funnel`'s stage names, `host.whoami.activation`'s keys and
/// `/status.json.onboarding_7d`'s `stamped_n` keys are all read from this
/// one table, so a fifth stamp added here appears on all three surfaces.
pub const ACTIVATION_MILESTONES: &[(&str, &str)] = &[
    ("first_call", "host.quickstart"),
    ("first_publish", "host.tool_publish"),
    ("first_own_call", "host.tool_call"),
    ("second_session", ""),
];

/// `admin.funnel` stages that come after the activation milestones; not
/// tenant-visible, so not in [`ACTIVATION_MILESTONES`].
pub const ADMIN_ONLY_STAGES: &[&str] = &["claimed", "paid"];

/// Fewest external tenants before `onboarding_7d` shows percentiles (the PRD's
/// drafted minimum).
pub const ONBOARDING_MIN_N: usize = 5;

/// Nearest-rank percentile of an ascending-sorted, non-empty slice.
fn nearest_rank(sorted: &[i64], pct: f64) -> i64 {
    let rank = ((sorted.len() as f64) * pct / 100.0).ceil() as usize;
    sorted[rank.saturating_sub(1).min(sorted.len() - 1)]
}

fn stamp_of(row: &crate::db::OnboardingRow, milestone: &str) -> Option<i64> {
    match milestone {
        "first_call" => row.first_call_unix,
        "first_publish" => row.first_publish_unix,
        "first_own_call" => row.first_own_call_unix,
        "second_session" => row.second_session_unix,
        other => unreachable!("unknown milestone {other}: add it to stamp_of"),
    }
}

/// Seconds from `created` to each milestone's stamp (null when unstamped), in
/// [`ACTIVATION_MILESTONES`] order.
pub fn activation_seconds(
    created: Option<i64>,
    stamps: [Option<i64>; 4],
) -> Vec<(&'static str, Option<i64>)> {
    ACTIVATION_MILESTONES
        .iter()
        .zip(stamps)
        .map(|((name, _), stamp)| (*name, created.zip(stamp).map(|(c, s)| (s - c).max(0))))
        .collect()
}

/// R2: `host.whoami.activation` -- `<milestone>_s` seconds since birth (null
/// until stamped, never 0) for each [`ACTIVATION_MILESTONES`] entry, plus
/// `next_milestone` (`{name, tool}` of the first unreached one; `tool` is null
/// for a milestone no tool reaches; the whole value is null once all are).
pub fn activation_json(created: Option<i64>, stamps: [Option<i64>; 4]) -> Value {
    let seconds = activation_seconds(created, stamps);
    let mut out = serde_json::Map::new();
    for (name, secs) in &seconds {
        out.insert(format!("{name}_s"), json!(secs));
    }
    let next = ACTIVATION_MILESTONES.iter().zip(stamps).find(|(_, stamp)| stamp.is_none()).map(|((name, tool), _)| {
        json!({"name": name, "tool": (!tool.is_empty()).then_some(*tool)})
    });
    out.insert("next_milestone".into(), next.unwrap_or(Value::Null));
    Value::Object(out)
}

fn deltas(rows: &[&crate::db::OnboardingRow], milestone: &str) -> Vec<i64> {
    let mut v: Vec<i64> = rows
        .iter()
        .filter_map(|r| stamp_of(r, milestone).map(|s| (s - r.created_unix).max(0)))
        .collect();
    v.sort_unstable();
    v
}

fn percentile_or_null(sorted: &[i64], pct: f64, enough: bool) -> Value {
    if enough && !sorted.is_empty() { json!(nearest_rank(sorted, pct)) } else { Value::Null }
}

/// The block's body for one group of rows.
fn onboarding_group(rows: &[&crate::db::OnboardingRow], min_n: usize) -> serde_json::Map<String, Value> {
    let enough = rows.len() >= min_n;
    let mut out = serde_json::Map::new();
    out.insert("n".into(), json!(rows.len()));
    let mut stamped = serde_json::Map::new();
    for (name, _) in ACTIVATION_MILESTONES {
        stamped.insert((*name).to_string(), json!(deltas(rows, name).len()));
    }
    for name in ["first_publish", "first_own_call"] {
        let d = deltas(rows, name);
        out.insert(format!("p50_s_to_{name}"), percentile_or_null(&d, 50.0, enough));
        out.insert(format!("p95_s_to_{name}"), percentile_or_null(&d, 95.0, enough));
    }
    let under = rows
        .iter()
        .filter(|r| r.first_own_call_unix.is_some_and(|s| s - r.created_unix <= 60))
        .count();
    let share = if enough && !rows.is_empty() {
        json!((under as f64 / rows.len() as f64 * 1000.0).round() / 1000.0)
    } else {
        Value::Null
    };
    out.insert("under_60s_share".into(), share);
    out.insert("stamped_n".into(), Value::Object(stamped));
    out
}

/// R3: `/status.json.onboarding_7d` over `origin = 'external'` tenants created
/// in the 7 days before `now`. Percentiles and `under_60s_share` are null
/// below `min_n` tenants (the public path passes [`ONBOARDING_MIN_N`]). Only
/// numbers and null leave this function (R4): no tenant, client or address.
pub async fn onboarding_7d(db: &Db, now: i64, min_n: usize) -> Result<Value, AppError> {
    let rows = db.onboarding_rows(now - 7 * 86_400).await?;
    let all: Vec<&crate::db::OnboardingRow> = rows.iter().collect();
    let mut block = onboarding_group(&all, min_n);
    // R6: the same body per client surface for groups of at least
    // `ONBOARDING_MIN_N` tenants. Names come from `SURFACE_IDS` (unknown or
    // missing -> `other`), never the raw `client_name`.
    let mut by_client = serde_json::Map::new();
    let mut groups: std::collections::BTreeMap<&'static str, Vec<&crate::db::OnboardingRow>> = Default::default();
    for row in &all {
        groups.entry(crate::install_links::surface_id(row.client_name.as_deref())).or_default().push(row);
    }
    for (surface, group) in groups {
        if group.len() >= ONBOARDING_MIN_N {
            by_client.insert(surface.to_string(), Value::Object(onboarding_group(&group, min_n)));
        }
    }
    block.insert("by_client".into(), Value::Object(by_client));
    block.insert("computed_at".into(), json!(now));
    Ok(Value::Object(block))
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
