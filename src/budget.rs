//! PRD-mcphost-run-budget-governor: a small standard-library budget ledger
//! over four dimensions (child calls, estimated tokens, tool latency, wall
//! clock), ported from ai-stack's `mcp-query-budget-governor` (`~/projects/
//! ai-stack` @ 4ef6c22) -- `BudgetLimits`/`BudgetLedger`/`Verdict` keep that
//! crate's own shape (requirement 1): each limit is `Option` (unconstrained
//! when `None`), `record` takes the same `(est_tokens, latency_ms)` pair,
//! `check(now_ms)` returns `Ok`/`Alert`/`Exceeded { dimension }`, and
//! `fraction_used(now_ms)` is the max fraction across whichever dimensions
//! carry a limit.
//!
//! `check`/`record` take `now_ms` as a plain argument (Technical
//! considerations: "the ledger takes `now_ms` as an argument, as in the
//! source crate, so tests inject time") rather than reading the clock
//! themselves -- this module has no `tokio`/wall-clock dependency at all.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use crate::errors::AppError;
use crate::plans::Plan;
use crate::state::AppState;

/// requirement 1: the default `checkin_fraction` -- an `Alert` fires once
/// any active dimension's fraction reaches 80% of its limit.
pub const DEFAULT_ALERT_FRACTION: f64 = 0.8;

/// Which of [`BudgetLedger`]'s four tracked quantities a [`Verdict::Exceeded`]
/// names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dimension {
    ChildCalls,
    EstTokens,
    ToolLatencyMs,
    WallMs,
}

impl Dimension {
    pub fn as_str(self) -> &'static str {
        match self {
            Dimension::ChildCalls => "child_calls",
            Dimension::EstTokens => "est_tokens",
            Dimension::ToolLatencyMs => "tool_latency_ms",
            Dimension::WallMs => "wall_ms",
        }
    }
}

/// `BudgetLedger::check`'s outcome: `Ok` under every active dimension's
/// alert threshold, `Alert` once any active dimension reaches
/// [`BudgetLimits::alert_fraction`] (default 0.8) but none has reached its
/// limit, `Exceeded` the moment any active dimension reaches (or passes)
/// its limit -- naming the first such dimension found, in the fixed order
/// `child_calls`, `est_tokens`, `tool_latency_ms`, `wall_ms`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    Alert,
    Exceeded { dimension: Dimension },
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Ok => "ok",
            Verdict::Alert => "alert",
            Verdict::Exceeded { .. } => "exceeded",
        }
    }
}

/// requirement 1: `BudgetLimits {max_child_calls, max_est_tokens,
/// max_tool_latency_ms, max_wall_ms, alert_fraction}`, each limit `Option`
/// (unconstrained when `None`). `max_child_calls` is capped at
/// [`crate::kinds::COMPOSE_CHILDREN_MAX`] by [`crate::plans::Plan::budget_defaults`],
/// not here -- this struct itself accepts whatever it's given, same as the
/// ai-stack source crate's own `BudgetLimits`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BudgetLimits {
    pub max_child_calls: Option<i64>,
    pub max_est_tokens: Option<i64>,
    pub max_tool_latency_ms: Option<i64>,
    pub max_wall_ms: Option<i64>,
    pub alert_fraction: f64,
}

impl BudgetLimits {
    /// Every dimension unconstrained, [`DEFAULT_ALERT_FRACTION`] -- a
    /// ledger under this never alerts or exceeds, regardless of use.
    pub fn unconstrained() -> Self {
        Self {
            max_child_calls: None,
            max_est_tokens: None,
            max_tool_latency_ms: None,
            max_wall_ms: None,
            alert_fraction: DEFAULT_ALERT_FRACTION,
        }
    }

    pub fn to_json(self) -> Value {
        json!({
            "max_child_calls": self.max_child_calls,
            "max_est_tokens": self.max_est_tokens,
            "max_tool_latency_ms": self.max_tool_latency_ms,
            "max_wall_ms": self.max_wall_ms,
            "alert_fraction": self.alert_fraction,
        })
    }

    pub fn from_json(v: &Value) -> Self {
        Self {
            max_child_calls: v.get("max_child_calls").and_then(Value::as_i64),
            max_est_tokens: v.get("max_est_tokens").and_then(Value::as_i64),
            max_tool_latency_ms: v.get("max_tool_latency_ms").and_then(Value::as_i64),
            max_wall_ms: v.get("max_wall_ms").and_then(Value::as_i64),
            alert_fraction: v
                .get("alert_fraction")
                .and_then(Value::as_f64)
                .unwrap_or(DEFAULT_ALERT_FRACTION),
        }
    }
}

/// requirement 1: `BudgetLedger {started_ms, child_calls, est_tokens,
/// tool_latency_ms}`. `wall_ms` isn't a stored field -- it's always derived
/// at check time as `now_ms - started_ms`, the same "current elapsed wall
/// clock" the source crate's own ledger computes rather than stores.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BudgetLedger {
    pub started_ms: i64,
    pub child_calls: i64,
    pub est_tokens: i64,
    pub tool_latency_ms: i64,
}

impl BudgetLedger {
    pub fn new(started_ms: i64) -> Self {
        Self {
            started_ms,
            child_calls: 0,
            est_tokens: 0,
            tool_latency_ms: 0,
        }
    }

    /// Records one completed child call: `child_calls` +1, `est_tokens` and
    /// `tool_latency_ms` each incremented by this call's own cost.
    pub fn record(&mut self, est_tokens: i64, latency_ms: i64) {
        self.child_calls += 1;
        self.est_tokens += est_tokens.max(0);
        self.tool_latency_ms += latency_ms.max(0);
    }

    fn fraction(value: i64, limit: Option<i64>) -> Option<f64> {
        limit.map(|l| if l <= 0 { 1.0 } else { value as f64 / l as f64 })
    }

    /// Per-dimension fractions currently active (the limit is `Some`), in
    /// the fixed order `check`'s `Exceeded` search uses too:
    /// `child_calls`, `est_tokens`, `tool_latency_ms`, `wall_ms`.
    fn active_fractions(&self, limits: &BudgetLimits, now_ms: i64) -> Vec<(Dimension, f64)> {
        let wall_ms = (now_ms - self.started_ms).max(0);
        [
            (Dimension::ChildCalls, Self::fraction(self.child_calls, limits.max_child_calls)),
            (Dimension::EstTokens, Self::fraction(self.est_tokens, limits.max_est_tokens)),
            (
                Dimension::ToolLatencyMs,
                Self::fraction(self.tool_latency_ms, limits.max_tool_latency_ms),
            ),
            (Dimension::WallMs, Self::fraction(wall_ms, limits.max_wall_ms)),
        ]
        .into_iter()
        .filter_map(|(d, f)| f.map(|f| (d, f)))
        .collect()
    }

    pub fn check(&self, limits: &BudgetLimits, now_ms: i64) -> Verdict {
        let fractions = self.active_fractions(limits, now_ms);
        if let Some((dimension, _)) = fractions.iter().find(|(_, f)| *f >= 1.0) {
            return Verdict::Exceeded { dimension: *dimension };
        }
        if fractions.iter().any(|(_, f)| *f >= limits.alert_fraction) {
            return Verdict::Alert;
        }
        Verdict::Ok
    }

    /// The max fraction across every active (limited) dimension -- `0.0`
    /// when no dimension carries a limit at all.
    pub fn fraction_used(&self, limits: &BudgetLimits, now_ms: i64) -> f64 {
        self.active_fractions(limits, now_ms)
            .into_iter()
            .map(|(_, f)| f)
            .fold(0.0, f64::max)
    }

    /// This ledger's own current value for `dimension`, and that
    /// dimension's limit (`i64::MAX` when unconstrained) -- used to build
    /// `Exceeded`'s `{dimension, limit, used}` error detail.
    pub fn used_and_limit(&self, limits: &BudgetLimits, dimension: Dimension, now_ms: i64) -> (i64, i64) {
        match dimension {
            Dimension::ChildCalls => (self.child_calls, limits.max_child_calls.unwrap_or(i64::MAX)),
            Dimension::EstTokens => (self.est_tokens, limits.max_est_tokens.unwrap_or(i64::MAX)),
            Dimension::ToolLatencyMs => {
                (self.tool_latency_ms, limits.max_tool_latency_ms.unwrap_or(i64::MAX))
            }
            Dimension::WallMs => {
                ((now_ms - self.started_ms).max(0), limits.max_wall_ms.unwrap_or(i64::MAX))
            }
        }
    }
}

/// requirement 3: `host.tool_call(..., budget?)` accepts any subset of the
/// four limits; a value above the plan default is a validation error naming
/// the ceiling (AC3) -- non-goal 3: "a per-call budget can only lower the
/// plan default", so this never raises a field above [`Plan::budget_defaults`].
/// `requested: None` (or JSON `null`) returns the plan defaults unchanged.
///
/// PRD-mcphost-upgrade-moment requirement 1: `budget_ceiling_exceeded` is
/// one of the five refusal sites that carries `next` (`state` is only
/// needed for that -- the plan catalog and billing mode).
pub fn resolve_and_validate(
    state: &AppState,
    plan: &Plan,
    requested: Option<&Value>,
) -> Result<BudgetLimits, AppError> {
    let defaults = plan.budget_defaults();
    let Some(requested) = requested.filter(|v| !v.is_null()) else {
        return Ok(defaults);
    };
    let obj = requested
        .as_object()
        .ok_or_else(|| AppError::InvalidArgs("'budget' must be an object".to_string()))?;
    let mut limits = defaults;
    macro_rules! field {
        ($key:literal, $slot:ident) => {
            if let Some(v) = obj.get($key) {
                let requested_value = v.as_i64().filter(|n| *n > 0).ok_or_else(|| {
                    AppError::InvalidArgs(format!("'budget.{}' must be a positive integer", $key))
                })?;
                let ceiling = defaults.$slot.unwrap_or(i64::MAX);
                if requested_value > ceiling {
                    let next = crate::billing::next_upgrade(
                        &state.plans,
                        state.billing_config.billing_mode(),
                        &plan.name,
                        $key,
                        requested_value,
                        ceiling,
                        None,
                    );
                    return Err(AppError::Structured {
                        code: "budget_ceiling_exceeded",
                        message: format!(
                            "budget.{}: {requested_value} exceeds this plan's ceiling of {ceiling}",
                            $key
                        ),
                        data: json!({
                            "field": format!("budget.{}", $key),
                            "ceiling": ceiling,
                            "requested": requested_value,
                            "next": next,
                        }),
                    });
                }
                limits.$slot = Some(requested_value);
            }
        };
    }
    field!("max_child_calls", max_child_calls);
    field!("max_est_tokens", max_est_tokens);
    field!("max_tool_latency_ms", max_tool_latency_ms);
    field!("max_wall_ms", max_wall_ms);
    Ok(limits)
}

/// Technical considerations: "the estimate is `chars / 4` over arguments
/// and results the host sees" -- `ceil((args_bytes + result_bytes) / 4)`,
/// the footprint meter's own default, computed over each value's
/// `serde_json`-serialized byte length (the same "chars" a caller actually
/// sent/received, not a model-tokenizer count -- Non-goals: "not for
/// metering").
pub fn estimate_tokens(args: &Value, result: &Value) -> i64 {
    let args_bytes = serde_json::to_string(args).map(|s| s.len()).unwrap_or(0);
    let result_bytes = serde_json::to_string(result).map(|s| s.len()).unwrap_or(0);
    ((args_bytes + result_bytes) as i64 + 3) / 4
}

/// requirement 4/5: the live, per-run handle every composed child call
/// checks and records against, and [`crate::runs`]'s executor persists into
/// the run row. Shared (via the `Arc` every [`crate::kinds::CallCtx::budget`]
/// clone holds) across a whole call tree, same propagation shape
/// `CallCtx::compose_children` already uses.
pub struct BudgetTracker {
    limits: BudgetLimits,
    ledger: Mutex<BudgetLedger>,
    /// 0 = not yet alerted; otherwise the unix-seconds timestamp of the
    /// first `Alert`/`Exceeded` verdict this tracker ever saw (requirement
    /// 6: "later alerts do not repeat").
    alerted_at_unix: AtomicI64,
    state: crate::state::AppState,
    tenant_id: i64,
    run_id: String,
    /// PRD-mcphost-upgrade-moment requirement 1: the tenant's plan name at
    /// construction time, so [`Self::exceeded_detail`]'s `next` block can
    /// name the next plan up without a DB round trip back through
    /// `tenant_id` (this tracker already outlives the `Tenant` its own
    /// caller resolved it from).
    plan: String,
}

impl BudgetTracker {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        state: crate::state::AppState,
        tenant_id: i64,
        plan: String,
        run_id: String,
        limits: BudgetLimits,
        now_ms: i64,
    ) -> Arc<Self> {
        Arc::new(Self {
            limits,
            ledger: Mutex::new(BudgetLedger::new(now_ms)),
            alerted_at_unix: AtomicI64::new(0),
            state,
            tenant_id,
            run_id,
            plan,
        })
    }

    pub fn check(&self, now_ms: i64) -> Verdict {
        self.ledger.lock().unwrap_or_else(|e| e.into_inner()).check(&self.limits, now_ms)
    }

    pub fn record(&self, est_tokens: i64, latency_ms: i64) {
        self.ledger.lock().unwrap_or_else(|e| e.into_inner()).record(est_tokens, latency_ms);
    }

    /// requirement 4's `Exceeded` error detail: `{dimension, limit, used}`,
    /// plus (PRD-mcphost-upgrade-moment requirement 1) `next`.
    pub fn exceeded_detail(&self, dimension: Dimension, now_ms: i64) -> Value {
        let ledger = self.ledger.lock().unwrap_or_else(|e| e.into_inner());
        let (used, limit) = ledger.used_and_limit(&self.limits, dimension, now_ms);
        let next = crate::billing::next_upgrade(
            &self.state.plans,
            self.state.billing_config.billing_mode(),
            &self.plan,
            dimension.as_str(),
            used,
            limit,
            None,
        );
        json!({"dimension": dimension.as_str(), "limit": limit, "used": used, "next": next})
    }

    /// requirement 5: `{limits, used, fraction, verdict}`, plus
    /// `alerted_at_unix` once this tracker has ever alerted.
    fn snapshot(&self, now_ms: i64) -> Value {
        let ledger = self.ledger.lock().unwrap_or_else(|e| e.into_inner());
        let verdict = ledger.check(&self.limits, now_ms);
        let fraction = ledger.fraction_used(&self.limits, now_ms);
        let wall_ms = (now_ms - ledger.started_ms).max(0);
        let mut snapshot = json!({
            "limits": self.limits.to_json(),
            "used": {
                "child_calls": ledger.child_calls,
                "est_tokens": ledger.est_tokens,
                "tool_latency_ms": ledger.tool_latency_ms,
                "wall_ms": wall_ms,
            },
            "fraction": fraction,
            "verdict": verdict.as_str(),
        });
        let alerted_at = self.alerted_at_unix.load(Ordering::SeqCst);
        if alerted_at > 0
            && let Some(obj) = snapshot.as_object_mut()
        {
            obj.insert("alerted_at_unix".to_string(), json!(alerted_at));
        }
        snapshot
    }

    /// requirement 5: merges this tracker's current snapshot into the run
    /// row's `progress_json.budget` (preserving whatever else -- e.g.
    /// `mcphost.progress`'s own `pct`/`msg` -- already lives there).
    /// requirement 6 / AC8: on the FIRST tick this tracker observes
    /// `Alert`/`Exceeded`, stamps `alerted_at_unix` and raises one
    /// `run.budget_alert` event on the alert module's own store; every
    /// later call (however many further `Alert`/`Exceeded` ticks this run
    /// has) only repeats the stamp, never a second event.
    pub async fn persist(&self, now_ms: i64) {
        let verdict = self.check(now_ms);
        let just_alerted = matches!(verdict, Verdict::Alert | Verdict::Exceeded { .. })
            && self
                .alerted_at_unix
                .compare_exchange(0, crate::state::now_unix(), Ordering::SeqCst, Ordering::SeqCst)
                .is_ok();
        let snapshot = self.snapshot(now_ms);

        if let Ok(Some(run)) = self.state.db.get_run(self.run_id.clone(), self.tenant_id).await {
            let mut progress: Value = run
                .progress_json
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_else(|| json!({}));
            if !progress.is_object() {
                progress = json!({});
            }
            if let Some(obj) = progress.as_object_mut() {
                obj.insert("budget".to_string(), snapshot.clone());
            }
            let progress_json = progress.to_string();
            let _ = self.state.db.update_run_progress(self.run_id.clone(), self.tenant_id, progress_json).await;
        }

        if just_alerted {
            let _ = crate::alerts::raise(
                &self.state,
                crate::alerts::RaiseInput {
                    key: format!("run.budget_alert:{}", self.run_id),
                    severity: crate::alerts::Severity::Warn,
                    title: format!("run {} crossed its budget alert threshold", self.run_id),
                    body: json!({
                        "run_id": self.run_id,
                        "tenant_id": self.tenant_id,
                        "budget": snapshot,
                    }),
                },
            )
            .await;
        }
    }
}
