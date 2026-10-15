//! The `Kind` conformance suite (PRD requirement 5, AC17): a reusable check
//! any `Kind` implementation — this crate's `echo`, and the REST-wrapper and
//! code kinds shipped by the feature PRDs that extend this crate's
//! registry — must pass. Exposed from the library so those crates can run
//! it against their own kinds in their own copy of this crate's
//! `tests/ac17_kind_conformance.rs`, per the PRD's technical considerations.

use serde_json::Value;

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::contract::{self, KindContract, SourcePosition, codes, family};
use super::{CallCtx, Kind, KindError, SecretResolver};

/// What went wrong, and which `Kind` method is responsible — so a failure
/// message can name the method, as AC17 requires ("the suite fails naming
/// `describe`").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConformanceFailure {
    pub method: &'static str,
    pub reason: String,
}

impl std::fmt::Display for ConformanceFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.method, self.reason)
    }
}

/// Checks `validate` accepts `spec` and `describe`'s `input_schema` is
/// itself a valid JSON Schema (the case AC17 tests directly: "a `Kind`
/// whose `describe` returns an invalid schema").
pub fn check_schema(kind: &dyn Kind, spec: &Value) -> Result<(), ConformanceFailure> {
    kind.validate(spec).map_err(|e| ConformanceFailure {
        method: "validate",
        reason: e.to_string(),
    })?;

    let descriptor = kind.describe(spec);
    if descriptor.description.trim().is_empty() {
        return Err(ConformanceFailure {
            method: "describe",
            reason: "description must not be empty".to_string(),
        });
    }
    jsonschema::validator_for(&descriptor.input_schema).map_err(|e| ConformanceFailure {
        method: "describe",
        reason: format!("input_schema is not a valid JSON Schema: {e}"),
    })?;
    Ok(())
}

/// Checks that a call with arguments matching `describe`'s schema succeeds,
/// and that a call with `sample_bad_args` (arguments the schema rejects)
/// fails via [`super::KindError`], not by panicking.
pub async fn check_call(
    kind: &dyn Kind,
    spec: &Value,
    sample_good_args: Value,
    sample_bad_args: Value,
) -> Result<(), ConformanceFailure> {
    let ctx = CallCtx::for_test(1, "t_conformance");

    let descriptor = kind.describe(spec);
    let validator =
        jsonschema::validator_for(&descriptor.input_schema).map_err(|e| ConformanceFailure {
            method: "describe",
            reason: format!("input_schema is not a valid JSON Schema: {e}"),
        })?;
    if !validator.is_valid(&sample_good_args) {
        return Err(ConformanceFailure {
            method: "check_call",
            reason: "sample_good_args does not satisfy describe's own schema".to_string(),
        });
    }

    kind.call(spec, sample_good_args, &ctx)
        .await
        .map_err(|e| ConformanceFailure {
            method: "call",
            reason: format!("call with schema-valid arguments failed: {e}"),
        })?;

    if validator.is_valid(&sample_bad_args) {
        // Not every kind's schema can reject something; skip silently rather
        // than force every caller to hand-craft an invalid case.
        return Ok(());
    }
    match kind.call(spec, sample_bad_args, &ctx).await {
        Err(_) => Ok(()),
        Ok(_) => Err(ConformanceFailure {
            method: "call",
            reason: "call accepted arguments describe's schema rejects".to_string(),
        }),
    }
}

/// AC5 (PRD-mcphost-publish-first-try, non-functional: "the AC17 kind
/// conformance suite extended with the structured-error contract for every
/// kind"): checks that `validate(bad_spec)` fails, and that the resulting
/// [`super::KindError`], run through the same `AppError` conversion the
/// live server uses (`errors::AppError::from(KindError)` ->
/// `into_error_data`), yields the structured rejection shape requirement 2
/// promises -- `field` and `expected` present, not just a bare
/// `error_code` -- rather than only spot-checking a handful of cases over
/// the wire the way `tests/publishfirsttry_ac02_structured_error_fields.rs`
/// does. Every kind's `validate` message already follows the shared
/// "`<field>: <expected>`" convention (see `errors::AppError::split_field`),
/// so no kind-specific code is needed here: a kind gains this coverage for
/// free by calling this function once per invalid-spec case in its own
/// conformance test.
pub fn check_rejection_shape(kind: &dyn Kind, bad_spec: &Value) -> Result<(), ConformanceFailure> {
    let kind_err = kind
        .validate(bad_spec)
        .err()
        .ok_or_else(|| ConformanceFailure {
            method: "validate",
            reason: format!("expected validate to reject spec {bad_spec}, it was accepted"),
        })?;
    let data = crate::errors::AppError::from(kind_err)
        .into_error_data()
        .data
        .unwrap_or(Value::Null);
    let has_str = |key: &str| {
        data.get(key)
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty())
    };
    if !has_str("field") || !has_str("expected") {
        return Err(ConformanceFailure {
            method: "validate",
            reason: format!(
                "rejection for spec {bad_spec} is a bare code, missing field/expected: {data}"
            ),
        });
    }
    if !has_str("docs") {
        return Err(ConformanceFailure {
            method: "validate",
            reason: format!("rejection for spec {bad_spec} is missing docs: {data}"),
        });
    }
    Ok(())
}

/// PRD-mcphost-sandbox-ready requirement 3 / AC7: reusable shape check for
/// the `sandbox_unavailable` rejection `host.tool_publish`, `host.tool_test`
/// and `host.tool_run` all return identically (`errors::AppError::
/// sandbox_unavailable`) when a kind's sandbox self-test is failing.
/// Unlike [`check_rejection_shape`] above, this doesn't run a `Kind`'s own
/// `validate` -- the rejection this checks is built by `control.rs`/
/// `handler.rs` directly from an injected [`crate::sandbox::SandboxStatus`]
/// *before* any kind-level validation runs (that's the point: no ast-check
/// process is spawned), so there is no `KindError` for a `Kind::validate`
/// call to produce here. "The suite is exercised by injecting an unready
/// status" (AC7's own words) means constructing a `SandboxStatus` with
/// `ready: false` directly, exactly as this function's callers do.
pub fn check_sandbox_unavailable_shape(
    status: &crate::sandbox::SandboxStatus,
) -> Result<(), ConformanceFailure> {
    if status.ready {
        return Err(ConformanceFailure {
            method: "sandbox_unavailable",
            reason: "check_sandbox_unavailable_shape must be called with an unready status"
                .to_string(),
        });
    }
    let error_data = crate::errors::AppError::sandbox_unavailable(status, 5).into_error_data();
    if error_data.message.trim().is_empty() {
        return Err(ConformanceFailure {
            method: "sandbox_unavailable",
            reason: "message must not be empty".to_string(),
        });
    }
    let data = error_data.data.unwrap_or(Value::Null);
    let code_ok = data.get("error_code").and_then(Value::as_str) == Some("sandbox_unavailable");
    if !code_ok {
        return Err(ConformanceFailure {
            method: "sandbox_unavailable",
            reason: format!("error_code must be 'sandbox_unavailable', got: {data}"),
        });
    }
    for field in ["mechanism", "detail", "docs"] {
        let present = data
            .get(field)
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty());
        if !present {
            return Err(ConformanceFailure {
                method: "sandbox_unavailable",
                reason: format!("data.{field} must be a non-empty string, got: {data}"),
            });
        }
    }
    // PRD-mcphost-first-publish-real-kind requirement 3 (AC2/AC6): `echo` is
    // a stub, never offered as a real fallback for a rejected sandboxed
    // publish -- `alternatives` names only `http` now.
    let alternatives_ok = data
        .get("alternatives")
        .and_then(Value::as_array)
        .is_some_and(|a| a.iter().any(|v| v == "http") && !a.iter().any(|v| v == "echo"));
    if !alternatives_ok {
        return Err(ConformanceFailure {
            method: "sandbox_unavailable",
            reason: format!("data.alternatives must include 'http' and never 'echo', got: {data}"),
        });
    }
    let retry_after_ok = data
        .get("retry_after_s")
        .and_then(Value::as_u64)
        .is_some_and(|s| (1..=30).contains(&s));
    if !retry_after_ok {
        return Err(ConformanceFailure {
            method: "sandbox_unavailable",
            reason: format!("data.retry_after_s must be present and in [1, 30], got: {data}"),
        });
    }
    Ok(())
}

// ---- generated per-kind contract suite (PRD-mcphost-kind-contract-
// conformance-generated) ---------------------------------------------------

/// How one of the five contract cases ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaseOutcome {
    Pass,
    /// Not run, with the reason the kind's contract states.
    Skip(String),
    Fail(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaseReport {
    pub case: &'static str,
    pub outcome: CaseOutcome,
}

struct ConformanceSecrets;
impl SecretResolver for ConformanceSecrets {
    fn resolve(&self, name: &str) -> Option<String> {
        (name == contract::CONFORMANCE_SECRET_NAME)
            .then(|| contract::CONFORMANCE_SECRET_VALUE.to_string())
    }
}

/// How long a call that answers `tool_building` is retried before the case
/// fails.
const BUILD_WAIT: Duration = Duration::from_secs(90);

async fn call_when_built(
    kind: &dyn Kind,
    spec: &Value,
    args: Value,
    ctx: &CallCtx,
) -> Result<Value, KindError> {
    let deadline = Instant::now() + BUILD_WAIT;
    loop {
        match kind.call(spec, args.clone(), ctx).await {
            Err(KindError::Structured { code: codes::TOOL_BUILDING, .. })
                if Instant::now() < deadline =>
            {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            other => return other,
        }
    }
}

fn contract_ctx() -> CallCtx {
    let mut ctx = CallCtx::for_test(1, "t_conformance");
    ctx.secrets = Arc::new(ConformanceSecrets);
    ctx
}

fn with_smallest_budget(contract: &KindContract, mut spec: Value) -> Value {
    if let (Some(dim), Some(obj)) = (
        contract.budgets.iter().find(|b| b.field == "timeout_s"),
        spec.as_object_mut(),
    ) {
        obj.insert(dim.field.to_string(), Value::from(dim.min));
    }
    spec
}

fn describe_failure(kind: &dyn Kind, case: &str, detail: impl std::fmt::Display) -> CaseOutcome {
    CaseOutcome::Fail(format!("kind={} case={case} {detail}", kind.name()))
}

/// The failing call's code and data, or a failure naming `kind` and `case`
/// when the call did not fail structurally with a code its contract lists.
fn contracted_error(
    kind: &dyn Kind,
    case: &str,
    result: Result<Value, KindError>,
) -> Result<(&'static str, Value), CaseOutcome> {
    match result {
        Ok(v) => Err(describe_failure(kind, case, format!("call succeeded, expected an error: {v}"))),
        Err(KindError::Structured { code, data, .. }) => {
            if kind.contract().has_code(code) {
                Ok((code, data))
            } else {
                Err(describe_failure(kind, case, format!("code={code} not in contract")))
            }
        }
        Err(other) => Err(describe_failure(kind, case, format!("error is not structured: {other}"))),
    }
}

async fn case_publish(kind: &dyn Kind, contract: &KindContract) -> CaseOutcome {
    match check_schema(kind, &(contract.fixtures.good_spec)()) {
        Ok(()) => CaseOutcome::Pass,
        Err(e) => describe_failure(kind, contract::CASE_PUBLISH, e),
    }
}

async fn case_call(kind: &dyn Kind, contract: &KindContract) -> CaseOutcome {
    let case = contract::CASE_CALL;
    let fx = &contract.fixtures;
    let spec = (fx.good_spec)();
    let ctx = contract_ctx();
    let descriptor = kind.describe(&spec);
    let validator = match jsonschema::validator_for(&descriptor.input_schema) {
        Ok(v) => v,
        Err(e) => return describe_failure(kind, case, format!("input_schema is not valid: {e}")),
    };
    let good = (fx.good_args)();
    if !validator.is_valid(&good) {
        return describe_failure(kind, case, "good_args does not satisfy describe's own schema");
    }
    if let Err(e) = call_when_built(kind, &spec, good, &ctx).await {
        return describe_failure(kind, case, format!("call with schema-valid arguments failed: {e}"));
    }
    let bad = (fx.bad_args)();
    if !validator.is_valid(&bad) && call_when_built(kind, &spec, bad, &ctx).await.is_ok() {
        return describe_failure(kind, case, "call accepted arguments describe's schema rejects");
    }
    CaseOutcome::Pass
}

async fn case_error_with_source_position(kind: &dyn Kind, contract: &KindContract) -> CaseOutcome {
    let case = contract::CASE_ERROR_WITH_SOURCE_POSITION;
    let Some(erroring) = contract.fixtures.erroring_spec else {
        return describe_failure(kind, case, "contract has no erroring_spec and no stated skip");
    };
    let ctx = contract_ctx();
    let result = call_when_built(kind, &erroring(), serde_json::json!({}), &ctx).await;
    let (code, data) = match contracted_error(kind, case, result) {
        Ok(ok) => ok,
        Err(outcome) => return outcome,
    };
    if contract.family_of(code) != Some(family::TOOL_CODE) {
        return CaseOutcome::Pass;
    }
    let missing = match contract.source_position {
        SourcePosition::None => None,
        SourcePosition::Line => (data.get("source_line").is_none()).then_some("source_line"),
        SourcePosition::LineColumn => ["source_line", "source_column"]
            .into_iter()
            .find(|k| data.get(*k).is_none()),
        SourcePosition::Field(f) => data.get(f).is_none().then_some(f),
    };
    match missing {
        Some(field) => describe_failure(kind, case, format!("code={code} carries no data.{field}")),
        None => CaseOutcome::Pass,
    }
}

async fn case_budget_exceeded(kind: &dyn Kind, contract: &KindContract) -> CaseOutcome {
    let case = contract::CASE_BUDGET_EXCEEDED;
    let Some(slow) = contract.fixtures.slow_spec else {
        return describe_failure(kind, case, "contract has no slow_spec and no stated skip");
    };
    let spec = with_smallest_budget(contract, slow());
    let ctx = contract_ctx();
    let result = call_when_built(kind, &spec, serde_json::json!({}), &ctx).await;
    match contracted_error(kind, case, result) {
        Ok((code, _)) if contract.family_of(code) == Some(family::TOOL_LIMIT) => CaseOutcome::Pass,
        Ok((code, _)) => describe_failure(kind, case, format!("code={code} is not in the tool_limit family")),
        Err(outcome) => outcome,
    }
}

async fn case_secret_redaction(kind: &dyn Kind, contract: &KindContract) -> CaseOutcome {
    let case = contract::CASE_SECRET_REDACTION;
    let Some(secret) = contract.fixtures.secret_spec else {
        return describe_failure(kind, case, "contract.secret_redaction=true but no secret_spec");
    };
    let ctx = contract_ctx();
    let rendered = match call_when_built(kind, &secret(), serde_json::json!({}), &ctx).await {
        Ok(v) => v.to_string(),
        Err(KindError::Structured { message, data, .. }) => format!("{message} {data}"),
        Err(other) => other.to_string(),
    };
    if rendered.contains(contract::CONFORMANCE_SECRET_VALUE) {
        describe_failure(kind, case, "the secret value appears in the call's output")
    } else {
        CaseOutcome::Pass
    }
}

/// Runs the five contract cases against `kind` and reports each one.
pub async fn run_contract_report(kind: &dyn Kind) -> Vec<CaseReport> {
    let contract = kind.contract();
    let stated_skip = |case: &str| {
        contract
            .fixtures
            .skip_reason(case)
            .map(|reason| CaseOutcome::Skip(reason.to_string()))
    };
    let mut reports = Vec::with_capacity(5);
    for case in [
        contract::CASE_PUBLISH,
        contract::CASE_CALL,
        contract::CASE_ERROR_WITH_SOURCE_POSITION,
        contract::CASE_BUDGET_EXCEEDED,
        contract::CASE_SECRET_REDACTION,
    ] {
        let outcome = if case == contract::CASE_SECRET_REDACTION && !contract.secret_redaction {
            CaseOutcome::Skip("contract.secret_redaction=false".to_string())
        } else if let Some(skip) = stated_skip(case) {
            skip
        } else {
            match case {
                contract::CASE_PUBLISH => case_publish(kind, contract).await,
                contract::CASE_CALL => case_call(kind, contract).await,
                contract::CASE_ERROR_WITH_SOURCE_POSITION => {
                    case_error_with_source_position(kind, contract).await
                }
                contract::CASE_BUDGET_EXCEEDED => case_budget_exceeded(kind, contract).await,
                _ => case_secret_redaction(kind, contract).await,
            }
        };
        reports.push(CaseReport { case, outcome });
    }
    reports
}

/// The generated conformance suite: every failing case of
/// [`run_contract_report`], as a [`ConformanceFailure`] whose reason names
/// `kind=<name> case=<case>`.
pub async fn run_contract(kind: &dyn Kind) -> Vec<ConformanceFailure> {
    run_contract_report(kind)
        .await
        .into_iter()
        .filter_map(|r| match r.outcome {
            CaseOutcome::Fail(reason) => Some(ConformanceFailure { method: r.case, reason }),
            _ => None,
        })
        .collect()
}
