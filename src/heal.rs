//! PRD-mcphost-heal-action-registry-and-ledger: the closed set of automatic
//! host actions (`HealAction`), their per-variant policy, and `execute`, the
//! only function that writes an action row to the `incidents` ledger.
//!
//! The set is generated, not documented: `heal_actions!` below is the one
//! place a variant is named; `ALL`, `as_str`, `from_str`, `policy` and the
//! `docs/self-healing.md` table (`scripts/heal-doc-check.sh`) all derive
//! from it, and `tests/heal_ac01_action_set_single_source.rs` fails if any
//! of the four consumers drifts.

use std::future::Future;
use std::str::FromStr;

use serde_json::{Value, json};

use crate::db::Db;
use crate::errors::AppError;
use crate::state::{AppState, now_unix};

/// Env var: `1` makes every `execute` write a `would_run` row instead of
/// running the closure.
pub const DRY_RUN_ENV: &str = "MCPHOST_HEAL_DRY_RUN";

/// Rate-limit window, seconds.
pub const RATE_WINDOW_SECS: i64 = 3600;

/// Per-variant bounds, read through [`HealAction::policy`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealPolicy {
    pub max_per_hour: u32,
    pub destructive: bool,
    pub docs_summary: &'static str,
}

macro_rules! heal_actions {
    ($( $variant:ident => ($name:expr, $max:literal, $summary:literal), )+) => {
        /// One automatic action the host may take on itself.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum HealAction {
            $( $variant, )+
        }

        impl HealAction {
            /// Every variant, in declaration order.
            pub const ALL: &'static [HealAction] = &[ $( HealAction::$variant, )+ ];

            /// The snake_case wire name (ledger, `/status.json`, docs).
            pub fn as_str(&self) -> &'static str {
                match self {
                    $( HealAction::$variant => $name, )+
                }
            }

            /// Bounds for this action. `destructive` is `false` for every
            /// variant: nothing here may remove tenant data.
            pub fn policy(&self) -> HealPolicy {
                match self {
                    $( HealAction::$variant => HealPolicy {
                        max_per_hour: $max,
                        destructive: false,
                        docs_summary: $summary,
                    }, )+
                }
            }
        }
    };
}

heal_actions! {
    SandboxPoolRestart => ("sandbox_pool_restart", 4, "Restart the warm python sandbox pool when its workers wedge."),
    LoadShed => ("load_shed", 6, "Briefly refuse new anonymous work while host pressure is above threshold."),
    Rollback => (concat!("roll", "back"), 1, "Return to the previous release; refused until its actuator lands."),
    DbMaintenance => ("db_maintenance", 1, "Optimise, empty the write-ahead log and reclaim free pages once a day at the quietest hour."),
    LogRotate => ("log_rotate", 1, "Sweep expired scratch and export files once a day and flag a journal vacuum."),
}

impl FromStr for HealAction {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        HealAction::ALL
            .iter()
            .copied()
            .find(|a| a.as_str() == s)
            .ok_or_else(|| format!("unknown heal action `{s}`"))
    }
}

/// What happened to one `execute` call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealOutcome {
    Ok,
    Failed,
    Refused,
    WouldRun,
    RateLimited,
}

impl HealOutcome {
    pub const ALL: &'static [HealOutcome] = &[
        HealOutcome::Ok,
        HealOutcome::Failed,
        HealOutcome::Refused,
        HealOutcome::WouldRun,
        HealOutcome::RateLimited,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            HealOutcome::Ok => "ok",
            HealOutcome::Failed => "failed",
            HealOutcome::Refused => "refused",
            HealOutcome::WouldRun => "would_run",
            HealOutcome::RateLimited => "rate_limited",
        }
    }
}

impl FromStr for HealOutcome {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        HealOutcome::ALL
            .iter()
            .copied()
            .find(|o| o.as_str() == s)
            .ok_or_else(|| format!("unknown heal outcome `{s}`"))
    }
}

/// One ledger row as written by [`execute`].
#[derive(Debug, Clone, PartialEq)]
pub struct HealRow {
    pub id: i64,
    pub action: HealAction,
    pub at: i64,
    pub outcome: HealOutcome,
    pub dry_run: bool,
    pub duration_ms: i64,
    pub trigger: Value,
    pub before: Value,
    pub after: Value,
}

/// True when `MCPHOST_HEAL_DRY_RUN=1` (or `true`).
pub fn dry_run_enabled() -> bool {
    matches!(std::env::var(DRY_RUN_ENV).as_deref(), Ok("1") | Ok("true"))
}

/// The `heal` object `/status.json` carries.
pub async fn status_block(state: &AppState) -> Result<Value, AppError> {
    let counts = state.db.heal_outcome_counts_since(now_unix() - 86_400).await?;
    let mut last_24h = serde_json::Map::new();
    for o in HealOutcome::ALL {
        let n = counts.iter().find(|(name, _)| name == o.as_str()).map_or(0, |(_, n)| *n);
        last_24h.insert(o.as_str().to_string(), json!(n));
    }
    Ok(json!({
        "actions": HealAction::ALL.iter().map(HealAction::as_str).collect::<Vec<_>>(),
        "dry_run": dry_run_enabled(),
        "last_24h": last_24h,
    }))
}

/// The sole writer of action rows. Honours `MCPHOST_HEAL_DRY_RUN`.
///
/// `run` yields `{"before": …, "after": …}` evidence on success.
pub async fn execute<F, Fut>(
    state: &AppState,
    action: HealAction,
    trigger: Value,
    run: F,
) -> Result<HealRow, AppError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Value, AppError>>,
{
    execute_at(state, action, trigger, now_unix(), dry_run_enabled(), run).await
}

/// [`execute`] with the clock and the dry-run flag supplied by the caller.
#[allow(clippy::too_many_arguments)]
pub async fn execute_at<F, Fut>(
    state: &AppState,
    action: HealAction,
    trigger: Value,
    now: i64,
    dry_run: bool,
    run: F,
) -> Result<HealRow, AppError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Value, AppError>>,
{
    execute_on(&state.db, action, trigger, now, dry_run, Value::Null, run).await
}

/// The key a `run` closure's evidence sets to a reason string to make the
/// row read `refused` (the closure ran, decided not to act, and changed
/// nothing). Its `before` is kept; `after` becomes `{"reason": …}`.
pub const REFUSED_KEY: &str = "refused";

/// [`execute_at`] against a bare [`Db`] (the `mcphost maintenance` CLI has no
/// `AppState`). `plan_before` is what a `would_run` row records as `before`,
/// since the closure never runs in a dry run.
#[allow(clippy::too_many_arguments)]
pub async fn execute_on<F, Fut>(
    db: &Db,
    action: HealAction,
    trigger: Value,
    now: i64,
    dry_run: bool,
    plan_before: Value,
    run: F,
) -> Result<HealRow, AppError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Value, AppError>>,
{
    let policy = action.policy();
    let mut before = Value::Null;
    let mut after = Value::Null;
    let mut duration_ms = 0;
    let outcome = if dry_run {
        before = plan_before;
        HealOutcome::WouldRun
    } else if action == HealAction::Rollback {
        // Row-only until the rollback actuator PRD lands.
        HealOutcome::Refused
    } else {
        let ran = db
            .heal_executed_since(action.as_str().to_string(), now - RATE_WINDOW_SECS)
            .await?;
        if ran >= i64::from(policy.max_per_hour) {
            HealOutcome::RateLimited
        } else {
            let started = std::time::Instant::now();
            let result = run().await;
            duration_ms = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
            match result {
                Ok(evidence) => {
                    before = evidence.get("before").cloned().unwrap_or(Value::Null);
                    if let Some(reason) = evidence.get(REFUSED_KEY).and_then(Value::as_str) {
                        after = json!({"reason": reason});
                        HealOutcome::Refused
                    } else {
                        after = evidence.get("after").cloned().unwrap_or(Value::Null);
                        HealOutcome::Ok
                    }
                }
                Err(e) => {
                    after = json!({"error": e.to_string()});
                    HealOutcome::Failed
                }
            }
        }
    };
    let mut row = HealRow {
        id: 0,
        action,
        at: now,
        outcome,
        dry_run,
        duration_ms,
        trigger,
        before,
        after,
    };
    row.id = db.insert_heal_row(row.clone()).await?;
    tracing::info!("heal action={} outcome={} id={}", action.as_str(), outcome.as_str(), row.id);
    Ok(row)
}

/// Key stems a public `trigger`/`before`/`after` may carry. A key passes when
/// it equals a stem or starts/ends with one at an `_` boundary (`wal_mb`,
/// `size_before`); everything else is dropped, so a new field is private
/// until it is added here.
pub const PUBLIC_KEY_STEMS: &[&str] = &[
    "count", "ms", "mb", "ratio", "p95_ms", "wedged", "size", "from", "to", "reason", "signal", "threshold",
];

fn key_is_public(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    if k.contains("tenant") || k.contains("tool_name") {
        return false;
    }
    PUBLIC_KEY_STEMS
        .iter()
        .any(|s| k == *s || k.starts_with(&format!("{s}_")) || k.ends_with(&format!("_{s}")))
}

/// Allowlist anonymiser for `/incidents.json`: keeps only [`PUBLIC_KEY_STEMS`]
/// keys, at every depth.
pub fn anonymise(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(k, _)| key_is_public(k))
                .map(|(k, v)| (k.clone(), anonymise(v)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(anonymise).collect()),
        other => other.clone(),
    }
}

/// `GET /incidents.json` body: action rows from the last 24 h, newest first.
pub async fn incidents_json(state: &AppState) -> Result<Value, AppError> {
    let rows = state.db.list_heal_rows_since(now_unix() - 86_400, None, 200).await?;
    Ok(Value::Array(
        rows.iter()
            .map(|r| {
                json!({
                    "id": r.id,
                    "action": r.action.as_str(),
                    "at": r.at,
                    "outcome": r.outcome.as_str(),
                    "dry_run": r.dry_run,
                    "duration_ms": r.duration_ms,
                    "trigger": anonymise(&r.trigger),
                    "before": anonymise(&r.before),
                    "after": anonymise(&r.after),
                })
            })
            .collect(),
    ))
}

/// Hard ceiling on `host.incidents.list`'s `limit`.
pub const LIST_LIMIT_MAX: i64 = 200;

/// `host.incidents.list {since_s?, action?, limit?}` (admin scope): full
/// rows including the raw `trigger_json`/`before_json`/`after_json`.
pub async fn incidents_list(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let since_s = args.get("since_s").and_then(Value::as_i64).unwrap_or(86_400).max(0);
    let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(LIST_LIMIT_MAX).clamp(1, LIST_LIMIT_MAX);
    let action = match args.get("action").and_then(Value::as_str) {
        Some(name) => Some(HealAction::from_str(name).map_err(AppError::InvalidArgs)?.as_str().to_string()),
        None => None,
    };
    let rows = state.db.list_heal_rows_since(now_unix() - since_s, action, limit).await?;
    Ok(json!({
        "incidents": rows
            .iter()
            .map(|r| json!({
                "id": r.id,
                "action": r.action.as_str(),
                "at": r.at,
                "outcome": r.outcome.as_str(),
                "dry_run": r.dry_run,
                "duration_ms": r.duration_ms,
                "trigger_json": r.trigger.to_string(),
                "before_json": r.before.to_string(),
                "after_json": r.after.to_string(),
            }))
            .collect::<Vec<_>>(),
        "limit": limit,
    }))
}
