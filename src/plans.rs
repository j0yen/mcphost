//! The plan catalog (`plans.toml`): per-plan price and quotas. PRD
//! requirement 1: "The file is data; no quota number lives in code" -- the
//! two defaults below exist only to seed a fresh data dir the first time
//! `mcphost serve` runs against it (AC1); every quota check elsewhere in
//! this crate reads a [`Plan`] out of a loaded [`PlanCatalog`], never one
//! of these numbers directly.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::errors::AppError;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Plan {
    pub name: String,
    pub price_usd_month: i64,
    pub tools_max: i64,
    pub calls_per_day: i64,
    pub secrets_max: i64,
    pub description: String,
    /// PRD-mcphost-tenant-state requirement 4: total stored bytes across a
    /// tenant's whole state (KV namespace + every declared table's rows).
    pub state_bytes_max: i64,
    /// requirement 4: rows per declared table. The PRD names no baseline
    /// (only the byte quota gets an explicit default), so this is the
    /// builder's own choice, generous enough that the byte quota is the
    /// binding constraint for the user stories in the PRD (a handful of
    /// rows per monitored metric, hundreds for a staging/transform
    /// pipeline) well before the row count is.
    pub state_rows_max: i64,
    /// requirement 4: keys/rows touched by a single `host.state.*` (or,
    /// once the sandbox channel lands, `mcphost.state.*`) call; default 200
    /// per the PRD, same for every plan unless an operator edits
    /// `plans.toml`.
    pub state_ops_per_call_max: i64,
    /// PRD-mcphost-call-limits-honest requirement 3: how many calls this
    /// plan's tenants may have in flight at once, under the host-wide
    /// ceiling (`python`'s `DEFAULT_MAX_CONCURRENT_CALLS`). Defaults to `4`
    /// when a hand-edited `plans.toml` predates this key (see
    /// [`PlanBuilder::build`]) -- the same "additive, tolerant of an older
    /// file" parsing this crate already applies to every other field.
    pub concurrent_calls_per_tenant: i64,
    /// PRD-mcphost-sharing requirement 4 (AC6): how many of this plan's
    /// tenants' tools may be non-private (`'group'` or `'public'`
    /// visibility) at once. Defaults to `3` when a hand-edited
    /// `plans.toml` predates this key (see [`PlanBuilder::build`]), same
    /// tolerant-of-an-older-file convention as
    /// `concurrent_calls_per_tenant`.
    pub shared_tools_max: i64,
    /// PRD-mcphost-runs-and-jobs P0 requirement 4: the deadline (seconds) a
    /// job of this plan's tenants runs under, distinct from the ordinary
    /// call deadline ([`crate::state::CALL_TIMEOUT`]) -- the whole point of
    /// `async: true` is a longer budget than a synchronous call ever gets.
    /// Defaults to the `free` plan's own default (300) when a hand-edited
    /// `plans.toml` predates this key, same tolerant-parse convention as
    /// `concurrent_calls_per_tenant`/`shared_tools_max`.
    pub job_max_s: i64,
    /// requirement 4: how many of this plan's tenants' jobs may be
    /// `running` at once, under a host-wide ceiling
    /// ([`crate::runs::JOBS_HOST_CEILING`]). Defaults to `1` (the `free`
    /// plan's own default) when a hand-edited `plans.toml` predates this
    /// key.
    pub jobs_concurrent: i64,
    /// PRD-mcphost-schedules P0 requirement 4: how many `kind = 'schedule'`
    /// triggers this plan's tenants may have at once (`host.trigger.set`
    /// refuses a further one with `trigger_quota_exceeded` naming this).
    /// Defaults to the `free` plan's own default (3) when a hand-edited
    /// `plans.toml` predates this key, same tolerant-parse convention as
    /// `job_max_s`/`jobs_concurrent`.
    pub schedules_max: i64,
    /// requirement 4: the shortest gap (seconds) between two consecutive
    /// firings a schedule may declare on this plan (`host.trigger.set`
    /// refuses a shorter one with `trigger_interval_too_short` naming
    /// this). Defaults to the `free` plan's own default (300) when a
    /// hand-edited `plans.toml` predates this key.
    pub schedule_min_interval_s: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlanCatalog {
    pub plans: Vec<Plan>,
}

impl PlanCatalog {
    /// PRD-mcphost-metered-overage requirement 55: the published-numbers
    /// alignment (decided 2026-09-06) -- `free` (0, 50, 500, 2) and `pro`
    /// (19, 50, 100000, 20, with 50,000 calls/month included before usage
    /// billing) -- price_usd_month, tools_max, calls_per_day, secrets_max.
    /// mcphost-1 already carries this catalog as a hand-placed
    /// `plans.toml`, which [`Self::load_or_init`] honors over these
    /// defaults; this is only what seeds a *fresh* data dir (AC13).
    pub fn default_catalog() -> Self {
        Self {
            plans: vec![
                Plan {
                    name: "free".to_string(),
                    price_usd_month: 0,
                    tools_max: 50,
                    calls_per_day: 500,
                    secrets_max: 2,
                    description: "Free: 50 tools, 500 calls/day, 2 secrets. No card required."
                        .to_string(),
                    // PRD-mcphost-tenant-state requirement 4: "free 5 MiB".
                    state_bytes_max: 5 * 1024 * 1024,
                    state_rows_max: 10_000,
                    state_ops_per_call_max: 200,
                    concurrent_calls_per_tenant: 4,
                    // PRD-mcphost-sharing requirement 4 (AC6): "free 3".
                    shared_tools_max: 3,
                    // PRD-mcphost-runs-and-jobs open question, resolved at
                    // build: "free job_max_s 300 and jobs_concurrent 1".
                    job_max_s: 300,
                    jobs_concurrent: 1,
                    // PRD-mcphost-schedules open question, resolved at
                    // build: "free 3 schedules, 5-minute minimum".
                    schedules_max: 3,
                    schedule_min_interval_s: 300,
                },
                Plan {
                    name: "pro".to_string(),
                    price_usd_month: 19,
                    tools_max: 50,
                    calls_per_day: 100_000,
                    secrets_max: 20,
                    description: "Pro: 50 tools, 100,000 calls/day, 20 secrets. $19/month, \
                        50,000 calls included per month, then usage-billed."
                        .to_string(),
                    // PRD-mcphost-tenant-state requirement 4: "pro 200 MiB".
                    state_bytes_max: 200 * 1024 * 1024,
                    state_rows_max: 100_000,
                    state_ops_per_call_max: 200,
                    concurrent_calls_per_tenant: 10,
                    // PRD-mcphost-sharing requirement 4: "pro 50".
                    shared_tools_max: 50,
                    // PRD-mcphost-runs-and-jobs: pro gets a longer deadline
                    // and more concurrent jobs than free, same free/pro
                    // scaling shape as every other quota above.
                    job_max_s: 900,
                    jobs_concurrent: 3,
                    // PRD-mcphost-schedules requirement 4: "pro 25
                    // schedules, 1-minute minimum".
                    schedules_max: 25,
                    schedule_min_interval_s: 60,
                },
            ],
        }
    }

    pub fn get(&self, name: &str) -> Option<&Plan> {
        self.plans.iter().find(|p| p.name == name)
    }

    /// `$MCPHOST_PLANS_PATH`, defaulting to `<data_dir>/plans.toml`
    /// (requirement 1).
    pub fn path_from_env(data_dir: &Path) -> PathBuf {
        std::env::var("MCPHOST_PLANS_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| data_dir.join("plans.toml"))
    }

    /// AC1: if `path` doesn't exist yet, write [`Self::default_catalog`]
    /// there (so an operator can find and hand-edit it) and return it;
    /// otherwise parse whatever is already on disk.
    pub fn load_or_init(path: &Path) -> Result<Self, AppError> {
        if !path.exists() {
            let catalog = Self::default_catalog();
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| AppError::Storage(format!("plans.toml directory: {e}")))?;
            }
            std::fs::write(path, catalog.to_toml())
                .map_err(|e| AppError::Storage(format!("write plans.toml: {e}")))?;
            return Ok(catalog);
        }
        let text = std::fs::read_to_string(path)
            .map_err(|e| AppError::Storage(format!("read plans.toml: {e}")))?;
        Self::from_toml(&text)
    }

    /// A minimal, hand-written serializer for this crate's one fixed plan
    /// shape (an array of `[[plan]]` tables, string/integer fields only) --
    /// see [`Self::from_toml`]'s doc comment for why this isn't a general
    /// TOML writer (this crate has no `toml` dependency, the same
    /// rationale as `state::rfc3339_from_unix`'s hand-rolled calendar
    /// math).
    pub fn to_toml(&self) -> String {
        let mut out = String::new();
        for p in &self.plans {
            out.push_str("[[plan]]\n");
            out.push_str(&format!("name = {:?}\n", p.name));
            out.push_str(&format!("price_usd_month = {}\n", p.price_usd_month));
            out.push_str(&format!("tools_max = {}\n", p.tools_max));
            out.push_str(&format!("calls_per_day = {}\n", p.calls_per_day));
            out.push_str(&format!("secrets_max = {}\n", p.secrets_max));
            out.push_str(&format!("description = {:?}\n", p.description));
            out.push_str(&format!("state_bytes_max = {}\n", p.state_bytes_max));
            out.push_str(&format!("state_rows_max = {}\n", p.state_rows_max));
            out.push_str(&format!(
                "state_ops_per_call_max = {}\n",
                p.state_ops_per_call_max
            ));
            out.push_str(&format!(
                "concurrent_calls_per_tenant = {}\n",
                p.concurrent_calls_per_tenant
            ));
            out.push_str(&format!("shared_tools_max = {}\n", p.shared_tools_max));
            out.push_str(&format!("job_max_s = {}\n", p.job_max_s));
            out.push_str(&format!("jobs_concurrent = {}\n", p.jobs_concurrent));
            out.push_str(&format!("schedules_max = {}\n", p.schedules_max));
            out.push_str(&format!(
                "schedule_min_interval_s = {}\n",
                p.schedule_min_interval_s
            ));
            out.push('\n');
        }
        out
    }

    /// Parses exactly the shape [`Self::to_toml`] writes: one or more
    /// `[[plan]]` tables, each a run of `key = value` lines (a
    /// double-quoted string or a bare integer), blank lines and `#`
    /// comments ignored. An operator who hand-edits `plans.toml` (Open
    /// Questions: "confirm or change in plans.toml before the first
    /// listing") must stick to this shape -- this is not a general TOML
    /// parser, deliberately, to avoid a new dependency for a file this
    /// crate itself writes the only complex form of.
    pub fn from_toml(text: &str) -> Result<Self, AppError> {
        let mut plans = Vec::new();
        let mut current: Option<PlanBuilder> = None;
        for raw_line in text.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line == "[[plan]]" {
                if let Some(builder) = current.take() {
                    plans.push(builder.build()?);
                }
                current = Some(PlanBuilder::default());
                continue;
            }
            let Some(builder) = current.as_mut() else {
                continue; // ignore stray lines before the first table
            };
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim();
            let str_value = || value.trim_matches('"').to_string();
            let int_value = || value.parse::<i64>().unwrap_or(0);
            match key {
                "name" => builder.name = Some(str_value()),
                "price_usd_month" => builder.price_usd_month = Some(int_value()),
                "tools_max" => builder.tools_max = Some(int_value()),
                "calls_per_day" => builder.calls_per_day = Some(int_value()),
                "secrets_max" => builder.secrets_max = Some(int_value()),
                "description" => builder.description = Some(str_value()),
                "state_bytes_max" => builder.state_bytes_max = Some(int_value()),
                "state_rows_max" => builder.state_rows_max = Some(int_value()),
                "state_ops_per_call_max" => builder.state_ops_per_call_max = Some(int_value()),
                "concurrent_calls_per_tenant" => {
                    builder.concurrent_calls_per_tenant = Some(int_value())
                }
                "shared_tools_max" => builder.shared_tools_max = Some(int_value()),
                "job_max_s" => builder.job_max_s = Some(int_value()),
                "jobs_concurrent" => builder.jobs_concurrent = Some(int_value()),
                "schedules_max" => builder.schedules_max = Some(int_value()),
                "schedule_min_interval_s" => {
                    builder.schedule_min_interval_s = Some(int_value())
                }
                _ => {}
            }
        }
        if let Some(builder) = current.take() {
            plans.push(builder.build()?);
        }
        if plans.is_empty() {
            return Err(AppError::Storage(
                "plans.toml has no [[plan]] entries".to_string(),
            ));
        }
        Ok(Self { plans })
    }
}

#[derive(Default)]
struct PlanBuilder {
    name: Option<String>,
    price_usd_month: Option<i64>,
    tools_max: Option<i64>,
    calls_per_day: Option<i64>,
    secrets_max: Option<i64>,
    description: Option<String>,
    state_bytes_max: Option<i64>,
    state_rows_max: Option<i64>,
    state_ops_per_call_max: Option<i64>,
    concurrent_calls_per_tenant: Option<i64>,
    shared_tools_max: Option<i64>,
    job_max_s: Option<i64>,
    jobs_concurrent: Option<i64>,
    schedules_max: Option<i64>,
    schedule_min_interval_s: Option<i64>,
}

impl PlanBuilder {
    fn build(self) -> Result<Plan, AppError> {
        Ok(Plan {
            name: self
                .name
                .ok_or_else(|| AppError::Storage("plans.toml: a [[plan]] is missing name".into()))?,
            price_usd_month: self.price_usd_month.unwrap_or(0),
            tools_max: self.tools_max.unwrap_or(0),
            calls_per_day: self.calls_per_day.unwrap_or(0),
            secrets_max: self.secrets_max.unwrap_or(0),
            description: self.description.unwrap_or_default(),
            state_bytes_max: self.state_bytes_max.unwrap_or(0),
            state_rows_max: self.state_rows_max.unwrap_or(0),
            state_ops_per_call_max: self.state_ops_per_call_max.unwrap_or(0),
            // Requirement 3: a `plans.toml` hand-edited (or generated)
            // before this PRD has no `concurrent_calls_per_tenant` line at
            // all -- `4` (the `free` plan's own default) keeps such a file
            // loadable rather than failing `load_or_init` outright.
            concurrent_calls_per_tenant: self.concurrent_calls_per_tenant.unwrap_or(4),
            // PRD-mcphost-sharing requirement 4: a `plans.toml` predating
            // this key gets the `free` plan's own default (3), same
            // tolerant-parse rationale as `concurrent_calls_per_tenant`.
            shared_tools_max: self.shared_tools_max.unwrap_or(3),
            // PRD-mcphost-runs-and-jobs: a `plans.toml` predating these two
            // keys gets the `free` plan's own defaults, same
            // tolerant-parse rationale as every other field above.
            job_max_s: self.job_max_s.unwrap_or(300),
            jobs_concurrent: self.jobs_concurrent.unwrap_or(1),
            // PRD-mcphost-schedules: a `plans.toml` predating these two
            // keys gets the `free` plan's own defaults, same
            // tolerant-parse rationale as every other field above.
            schedules_max: self.schedules_max.unwrap_or(3),
            schedule_min_interval_s: self.schedule_min_interval_s.unwrap_or(300),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PRD-mcphost-metered-overage AC13: the published-numbers alignment,
    /// not grand-loop-billing's original defaults.
    #[test]
    fn default_catalog_has_free_and_pro_with_documented_values() {
        let catalog = PlanCatalog::default_catalog();
        let free = catalog.get("free").expect("free plan");
        assert_eq!(free.price_usd_month, 0);
        assert_eq!(free.tools_max, 50);
        assert_eq!(free.calls_per_day, 500);
        assert_eq!(free.secrets_max, 2);
        let pro = catalog.get("pro").expect("pro plan");
        assert_eq!(pro.price_usd_month, 19);
        assert_eq!(pro.tools_max, 50);
        assert_eq!(pro.calls_per_day, 100_000);
        assert_eq!(pro.secrets_max, 20);
        assert!(
            pro.description.contains("50,000"),
            "pro's description must name the included monthly call volume: {}",
            pro.description
        );
    }

    /// PRD-mcphost-tenant-state requirement 4: "free 5 MiB, pro 200 MiB
    /// by default".
    #[test]
    fn default_catalog_has_state_quotas() {
        let catalog = PlanCatalog::default_catalog();
        let free = catalog.get("free").expect("free plan");
        assert_eq!(free.state_bytes_max, 5 * 1024 * 1024);
        assert_eq!(free.state_ops_per_call_max, 200);
        let pro = catalog.get("pro").expect("pro plan");
        assert_eq!(pro.state_bytes_max, 200 * 1024 * 1024);
        assert_eq!(pro.state_ops_per_call_max, 200);
        assert!(pro.state_rows_max > free.state_rows_max);
    }

    /// PRD-mcphost-runs-and-jobs open question, resolved at build: "free
    /// job_max_s 300 and jobs_concurrent 1".
    #[test]
    fn default_catalog_has_job_quotas() {
        let catalog = PlanCatalog::default_catalog();
        let free = catalog.get("free").expect("free plan");
        assert_eq!(free.job_max_s, 300);
        assert_eq!(free.jobs_concurrent, 1);
        let pro = catalog.get("pro").expect("pro plan");
        assert_eq!(pro.job_max_s, 900);
        assert_eq!(pro.jobs_concurrent, 3);
        assert!(pro.job_max_s > free.job_max_s);
        assert!(pro.jobs_concurrent > free.jobs_concurrent);
    }

    /// PRD-mcphost-schedules open question, resolved at build: "free 3
    /// schedules / 300s minimum, pro 25 schedules / 60s minimum".
    #[test]
    fn default_catalog_has_schedule_quotas() {
        let catalog = PlanCatalog::default_catalog();
        let free = catalog.get("free").expect("free plan");
        assert_eq!(free.schedules_max, 3);
        assert_eq!(free.schedule_min_interval_s, 300);
        let pro = catalog.get("pro").expect("pro plan");
        assert_eq!(pro.schedules_max, 25);
        assert_eq!(pro.schedule_min_interval_s, 60);
        assert!(pro.schedules_max > free.schedules_max);
        assert!(pro.schedule_min_interval_s < free.schedule_min_interval_s);
    }

    #[test]
    fn round_trips_through_toml() {
        let catalog = PlanCatalog::default_catalog();
        let text = catalog.to_toml();
        let parsed = PlanCatalog::from_toml(&text).expect("parse own output");
        assert_eq!(parsed, catalog);
    }

    #[test]
    fn load_or_init_writes_defaults_when_absent() {
        let dir = std::env::temp_dir().join(format!("mcphost-plans-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("plans.toml");
        let _ = std::fs::remove_file(&path);

        let catalog = PlanCatalog::load_or_init(&path).expect("load_or_init");
        assert!(path.exists());
        assert_eq!(catalog, PlanCatalog::default_catalog());

        // Second call reads the file back rather than rewriting it.
        let reloaded = PlanCatalog::load_or_init(&path).expect("reload");
        assert_eq!(reloaded, catalog);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unknown_lines_and_comments_are_ignored() {
        let text = "\
# a comment
[[plan]]
name = \"free\"
price_usd_month = 0
tools_max = 3
calls_per_day = 500
secrets_max = 2
description = \"Free tier\"
state_bytes_max = 1048576
state_rows_max = 100
state_ops_per_call_max = 50
made_up_key = \"ignored\"
";
        let catalog = PlanCatalog::from_toml(text).expect("parse");
        assert_eq!(catalog.plans.len(), 1);
        assert_eq!(catalog.plans[0].name, "free");
    }
}
