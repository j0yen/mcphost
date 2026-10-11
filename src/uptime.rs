//! PRD-mcphost-uptime-probe-recipe-green: `host.uptime.create(name, urls)` --
//! host-side uptime probing that needs no sandbox egress, so it works on the
//! free plan.
//!
//! One call creates the `<name>_targets` table, a probe tool `<name>_probe`,
//! a status tool `<name>` and one schedule on the probe, then runs the probe
//! once so the response already carries a table. Both tools are the internal
//! `uptime` kind below: the probe fetches every target through the registered
//! `http` kind (so it inherits that kind's SSRF policy, DNS vetting and
//! rate limit -- there is no second copy of "which hosts may be called"), and
//! the status tool only reads the last result the probe stored. Neither has a
//! `network` field, so the pro-plan egress gate never applies to them.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::future::join_all;
use serde_json::{Value, json};

use crate::errors::AppError;
use crate::kinds::{CallCtx, Kind, KindError, ToolDescriptor};

/// The kind name stored on both tools. Resolved by [`crate::kinds::KindRegistry::get`]
/// without being listed in [`crate::kinds::KindRegistry::names`]: it is
/// created only by `host.uptime.create`, never offered to `host.tool_publish`.
pub const KIND_NAME: &str = "uptime";

/// `host.uptime.create`'s target cap (PRD requirement 1).
pub const TARGETS_MAX: usize = 20;

/// Per-target deadline (PRD open question 2: 3 s, recorded in the PR).
pub const PER_TARGET_TIMEOUT: Duration = Duration::from_secs(3);

/// Default `every_s`: the free plan's floor.
pub const DEFAULT_EVERY_S: i64 = 300;

/// Cap on the `text` table a status call returns.
pub const TEXT_MAX_BYTES: usize = 2048;

/// The columns of a status result, in order.
pub const COLUMNS: [&str; 5] = ["url", "ok", "status", "latency_ms", "checked_at"];

pub fn targets_table(name: &str) -> String {
    format!("{name}_targets")
}

pub fn probe_tool_name(name: &str) -> String {
    format!("{name}_probe")
}

fn state_key(name: &str) -> String {
    format!("uptime:{name}")
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && s.len() <= 40
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// What `host.uptime.create` resolved from its arguments, before anything is
/// created.
#[derive(Debug, Clone)]
pub struct CreatePlan {
    pub name: String,
    pub urls: Vec<String>,
    pub cron: String,
    pub every_s: i64,
    pub clamped_to: Option<i64>,
}

/// Cron expression and its true period in seconds for a requested interval:
/// minutes are rounded up to a divisor of 60 (hours to a divisor of 24), so
/// every gap between fires is the same length and the schedule's own
/// "next two occurrences" interval check agrees with `every_s`.
fn cron_for(every_s: i64) -> (String, i64) {
    const MINUTE_STEPS: [i64; 12] = [1, 2, 3, 4, 5, 6, 10, 12, 15, 20, 30, 60];
    const HOUR_STEPS: [i64; 8] = [1, 2, 3, 4, 6, 8, 12, 24];
    let minutes = (every_s.max(1) + 59) / 60;
    if minutes <= 60 {
        let m = MINUTE_STEPS.iter().copied().find(|s| *s >= minutes).unwrap_or(60);
        let cron = match m {
            1 => "* * * * *".to_string(),
            60 => "0 * * * *".to_string(),
            _ => format!("*/{m} * * * *"),
        };
        return (cron, m * 60);
    }
    let hours = (minutes + 59) / 60;
    let h = HOUR_STEPS.iter().copied().find(|s| *s >= hours).unwrap_or(24);
    let cron = if h == 24 { "0 0 * * *".to_string() } else { format!("0 */{h} * * *") };
    (cron, h * 3600)
}

/// Validates `host.uptime.create`'s arguments against the tenant's plan and
/// the `http` kind's own spec validation. Pure: nothing is created.
pub fn parse_create(
    kinds: &crate::kinds::KindRegistry,
    plan: &crate::plans::Plan,
    args: &Value,
) -> Result<CreatePlan, AppError> {
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::InvalidArgs("missing required argument 'name'".to_string()))?
        .to_string();
    if !is_ident(&name) {
        return Err(AppError::InvalidArgs(format!(
            "name: must match ^[A-Za-z_][A-Za-z0-9_]{{0,39}}$; got '{name}'"
        )));
    }
    let urls_val = args
        .get("urls")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::InvalidArgs("missing required argument 'urls' (an array of 1..20 URLs)".to_string()))?;
    if urls_val.is_empty() {
        return Err(AppError::InvalidArgs("urls: empty -- give 1 to 20 URLs".to_string()));
    }
    if urls_val.len() > TARGETS_MAX {
        return Err(AppError::InvalidArgs(format!(
            "urls: max {TARGETS_MAX} URLs per probe set; got {}",
            urls_val.len()
        )));
    }
    let mut urls = Vec::with_capacity(urls_val.len());
    for v in urls_val {
        let url = v
            .as_str()
            .ok_or_else(|| AppError::InvalidArgs("urls: every entry must be a string".to_string()))?;
        urls.push(url.to_string());
    }
    if let Some(http) = kinds.get("http") {
        for url in &urls {
            let spec = json!({"method": "GET", "url": url});
            if let Err(e) = http.validate(&spec) {
                return Err(match e {
                    KindError::Structured { code: "host_not_allowed", message, data } => {
                        AppError::Structured {
                            code: "host_not_allowed",
                            message: format!("urls: '{url}' is not probeable ({message})"),
                            data: merge(data, json!({"url": url})),
                        }
                    }
                    other => AppError::InvalidArgs(format!("urls: '{url}': {other}")),
                });
            }
        }
    }

    let requested = args.get("every_s").and_then(Value::as_i64).unwrap_or(DEFAULT_EVERY_S);
    let floor = plan.schedule_min_interval_s;
    let effective = requested.max(floor);
    let (cron, every_s) = cron_for(effective);
    let clamped_to = (requested < floor).then_some(every_s);
    Ok(CreatePlan { name, urls, cron, every_s, clamped_to })
}

fn merge(base: Value, extra: Value) -> Value {
    let mut out = match base {
        Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    if let Value::Object(e) = extra {
        out.extend(e);
    }
    Value::Object(out)
}

/// Creates the table, both tools and the schedule. Returns the schedule's
/// trigger JSON. On any failure everything created so far is removed.
pub async fn create_assets(
    state: &crate::state::AppState,
    tenant: &crate::db::Tenant,
    plan: &CreatePlan,
) -> Result<Value, AppError> {
    let table = targets_table(&plan.name);
    crate::tables::table_create(state, tenant, &json!({"name": table, "columns": {"url": "text"}})).await?;
    let outcome = create_rest(state, tenant, plan, &table).await;
    if outcome.is_err() {
        let _ = crate::tables::table_drop(state, tenant, &json!({"name": table, "confirm": true})).await;
        for tool in [probe_tool_name(&plan.name), plan.name.clone()] {
            let _ = state.db.remove_tool(tenant.id, tool).await;
        }
    }
    outcome
}

async fn create_rest(
    state: &crate::state::AppState,
    tenant: &crate::db::Tenant,
    plan: &CreatePlan,
    table: &str,
) -> Result<Value, AppError> {
    let rows: Vec<Value> = plan.urls.iter().map(|u| json!({"url": u})).collect();
    crate::tables::table_append(state, tenant, &json!({"table": table, "rows": rows})).await?;
    let probe = probe_tool_name(&plan.name);
    for (tool, role) in [(&probe, "probe"), (&plan.name, "status")] {
        crate::control::tool_publish(
            state,
            tenant,
            &json!({"name": tool, "kind": KIND_NAME, "spec": {"role": role, "name": plan.name}}),
        )
        .await?;
    }
    crate::triggers::set(
        state,
        tenant,
        &json!({
            "tool": probe,
            "kind": "schedule",
            "schedule": plan.cron,
            "name": format!("uptime:{}", plan.name),
        }),
    )
    .await
}


// ---- plan numbers generated into www/llms.txt (PRD requirement 3) ----------

/// The plan numbers the llms.txt section quotes, as `<plan>.<field>` span
/// keys. Each is rendered between `<!-- uprg:KEY -->` and `<!-- /uprg:KEY -->`
/// by [`render_spans_into`] and checked by [`verify_spans`] -- the same
/// "generate the contract, never document it" shape as `gendocs`'s plan
/// tables, so the recipe's numbers cannot drift from `plans.rs`.
pub const SPAN_PLANS: [&str; 2] = ["free", "pro"];
pub const SPAN_FIELDS: [&str; 3] = ["schedules_max", "schedule_min_interval_s", "calls_per_day_at_floor"];

fn span_value(plan: &crate::plans::Plan, field: &str) -> Option<String> {
    Some(match field {
        "schedules_max" => plan.schedules_max.to_string(),
        "schedule_min_interval_s" => plan.schedule_min_interval_s.to_string(),
        "calls_per_day_at_floor" => (86_400 / plan.schedule_min_interval_s.max(1)).to_string(),
        _ => return None,
    })
}

/// Every `(key, expected value)` span the section carries.
pub fn expected_spans(catalog: &crate::plans::PlanCatalog) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for plan_name in SPAN_PLANS {
        let Some(plan) = catalog.plans.iter().find(|p| p.name == plan_name) else { continue };
        for field in SPAN_FIELDS {
            if let Some(v) = span_value(plan, field) {
                out.push((format!("{plan_name}.{field}"), v));
            }
        }
    }
    out
}

fn span_markers(key: &str) -> (String, String) {
    (format!("<!-- uprg:{key} -->"), format!("<!-- /uprg:{key} -->"))
}

/// Rewrites the value inside every expected span of `content`; a span that is
/// not present is left absent (so [`verify_spans`] reports it).
pub fn render_spans_into(content: &str, catalog: &crate::plans::PlanCatalog) -> String {
    let mut out = content.to_string();
    for (key, value) in expected_spans(catalog) {
        let (open, close) = span_markers(&key);
        let mut from = 0;
        while let Some(rel) = out[from..].find(&open) {
            let start = from + rel + open.len();
            let Some(end_rel) = out[start..].find(&close) else { break };
            out.replace_range(start..start + end_rel, &value);
            from = start + value.len() + close.len();
        }
    }
    out
}

/// `Ok` when every expected span is present at least once and every
/// occurrence equals `plans.rs`; otherwise the first mismatch, naming the span.
pub fn verify_spans(content: &str, catalog: &crate::plans::PlanCatalog) -> Result<(), String> {
    for (key, expected) in expected_spans(catalog) {
        let (open, close) = span_markers(&key);
        let mut found = 0;
        let mut from = 0;
        while let Some(rel) = content[from..].find(&open) {
            let start = from + rel + open.len();
            let end = content[start..]
                .find(&close)
                .ok_or_else(|| format!("span {key}: opening marker has no closing marker"))?;
            let actual = &content[start..start + end];
            if actual != expected {
                return Err(format!("span {key}: llms.txt says {actual:?}, plans.rs says {expected:?}"));
            }
            found += 1;
            from = start + end + close.len();
        }
        if found == 0 {
            return Err(format!("span {key}: missing from the section"));
        }
    }
    Ok(())
}

// ---- the kind -------------------------------------------------------------

/// The internal `uptime` kind: spec `{role: "probe"|"status", name}`.
pub struct UptimeKind {
    http: Option<Arc<dyn Kind>>,
}

impl UptimeKind {
    pub fn new(http: Option<Arc<dyn Kind>>) -> Self {
        Self { http }
    }
}

fn role_of(spec: &Value) -> &str {
    spec.get("role").and_then(Value::as_str).unwrap_or("")
}

fn name_of(spec: &Value) -> &str {
    spec.get("name").and_then(Value::as_str).unwrap_or("")
}

/// One target's row from the `http` kind's answer.
fn row_from(url: &str, outcome: Result<Result<Value, KindError>, tokio::time::error::Elapsed>, ms: u128, at: i64) -> Value {
    let (ok, status) = match outcome {
        Err(_) => (false, "timeout".to_string()),
        Ok(Ok(v)) => {
            let code = v.get("status").and_then(Value::as_u64).unwrap_or(0);
            ((200..400).contains(&code), code.to_string())
        }
        Ok(Err(KindError::Structured { code, data, .. })) => {
            let status = match (data.get("upstream_status").and_then(Value::as_u64), code) {
                (Some(s), _) => s.to_string(),
                (None, "upstream_timeout") => "timeout".to_string(),
                (None, "host_not_allowed") => "blocked".to_string(),
                (None, _) => "unreachable".to_string(),
            };
            (false, status)
        }
        Ok(Err(_)) => (false, "unreachable".to_string()),
    };
    json!({"url": url, "ok": ok, "status": status, "latency_ms": ms as i64, "checked_at": at})
}

async fn probe_all(http: &Arc<dyn Kind>, urls: &[String], ctx: &CallCtx) -> Vec<Value> {
    let at = crate::state::now_unix();
    let probes = urls.iter().map(|url| async move {
        let spec = json!({"method": "GET", "url": url, "timeout_s": PER_TARGET_TIMEOUT.as_secs()});
        let start = Instant::now();
        let outcome = tokio::time::timeout(PER_TARGET_TIMEOUT, http.call(&spec, json!({}), ctx)).await;
        row_from(url, outcome, start.elapsed().as_millis(), at)
    });
    join_all(probes).await
}

fn cell(row: &Value, col: &str) -> String {
    match row.get(col) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(v) => v.to_string(),
    }
}

/// The aligned, ≤ 2 KB table a status call returns as `text`.
pub fn render_text(rows: &[Value]) -> String {
    let shown: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            let checked = r
                .get("checked_at")
                .and_then(Value::as_i64)
                .map(crate::state::rfc3339_from_unix)
                .unwrap_or_default();
            vec![
                cell(r, "url"),
                if r.get("ok").and_then(Value::as_bool) == Some(true) { "up" } else { "DOWN" }.to_string(),
                cell(r, "status"),
                cell(r, "latency_ms"),
                checked,
            ]
        })
        .collect();
    let mut widths: Vec<usize> = COLUMNS.iter().map(|c| c.len()).collect();
    for row in &shown {
        for (w, c) in widths.iter_mut().zip(row) {
            *w = (*w).max(c.chars().count());
        }
    }
    let line = |cells: Vec<&str>| {
        cells
            .iter()
            .zip(&widths)
            .map(|(c, w)| format!("{c:<w$}"))
            .collect::<Vec<_>>()
            .join("  ")
            .trim_end()
            .to_string()
    };
    let mut lines = vec![line(COLUMNS.to_vec())];
    lines.extend(shown.iter().map(|r| line(r.iter().map(String::as_str).collect())));
    let mut text = lines.join("\n");
    if text.len() > TEXT_MAX_BYTES {
        let mut cut = TEXT_MAX_BYTES - 3;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push_str("...");
    }
    text
}

/// The probe's own `{columns, rows, text, checked_at}` out of whatever
/// envelope `host_tool_call` wrapped it in.
pub fn first_result_of(call_result: &Value) -> Value {
    for candidate in [call_result.get("result"), call_result.get("payload"), Some(call_result)] {
        if let Some(v) = candidate
            && v.get("rows").is_some()
        {
            return v.clone();
        }
    }
    call_result.clone()
}

fn result_value(rows: Vec<Value>, checked_at: Option<i64>) -> Value {
    json!({
        "columns": COLUMNS,
        "text": render_text(&rows),
        "rows": rows,
        "checked_at": checked_at,
    })
}

#[async_trait::async_trait]
impl Kind for UptimeKind {
    fn name(&self) -> &'static str {
        KIND_NAME
    }

    fn validate(&self, spec: &Value) -> Result<(), KindError> {
        if !matches!(role_of(spec), "probe" | "status") {
            return Err(KindError::InvalidSpec("spec.role: must be \"probe\" or \"status\"".into()));
        }
        if !is_ident(name_of(spec)) {
            return Err(KindError::InvalidSpec("spec.name: is required (the host.uptime.create name)".into()));
        }
        Ok(())
    }

    fn known_spec_fields(&self) -> &'static [&'static str] {
        &["role", "name"]
    }

    fn describe(&self, spec: &Value) -> ToolDescriptor {
        let description = match role_of(spec) {
            "probe" => "Probes every target of an uptime set and stores the result (created by host.uptime.create).",
            _ => "The last uptime probe result per target: {columns, rows, text} (created by host.uptime.create).",
        };
        ToolDescriptor {
            name: name_of(spec).to_string(),
            description: description.to_string(),
            input_schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
        }
    }

    async fn call(&self, spec: &Value, _args: Value, ctx: &CallCtx) -> Result<Value, KindError> {
        self.validate(spec)?;
        let name = name_of(spec);
        let key = state_key(name);
        if role_of(spec) == "status" {
            let stored = ctx.state.call("get", json!({"key": key})).await?;
            let value = stored.get("value").cloned().unwrap_or(Value::Null);
            let rows = value.get("rows").and_then(Value::as_array).cloned().unwrap_or_default();
            return Ok(result_value(rows, value.get("checked_at").and_then(Value::as_i64)));
        }

        let http = self.http.as_ref().ok_or_else(|| {
            KindError::structured("uptime_unavailable", "this host has no http kind registered to probe with")
        })?;
        let sql = format!("SELECT url FROM {} ORDER BY rowid LIMIT {TARGETS_MAX}", targets_table(name));
        let targets = ctx.table.call("query", json!({"sql": sql})).await?;
        let urls: Vec<String> = targets
            .get("rows")
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .filter_map(|r| r.get("url").and_then(Value::as_str).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let rows = probe_all(http, &urls, ctx).await;
        let checked_at = rows.first().and_then(|r| r.get("checked_at")).and_then(Value::as_i64);
        ctx.state
            .call("set", json!({"key": key, "value": {"checked_at": checked_at, "rows": rows}}))
            .await?;
        Ok(result_value(rows, checked_at))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cron_rounds_to_even_periods() {
        assert_eq!(cron_for(300), ("*/5 * * * *".to_string(), 300));
        assert_eq!(cron_for(60), ("* * * * *".to_string(), 60));
        assert_eq!(cron_for(420), ("*/10 * * * *".to_string(), 600));
        assert_eq!(cron_for(3600), ("0 * * * *".to_string(), 3600));
        assert_eq!(cron_for(7200), ("0 */2 * * *".to_string(), 7200));
    }

    #[test]
    fn text_stays_under_the_cap() {
        let rows: Vec<Value> = (0..20)
            .map(|i| json!({"url": format!("https://example.com/{}", "x".repeat(200 + i)), "ok": true, "status": "200", "latency_ms": 5, "checked_at": 0}))
            .collect();
        assert!(render_text(&rows).len() <= TEXT_MAX_BYTES);
    }
}
