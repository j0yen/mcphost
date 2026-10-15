//! AC1 — Given a test-only kind whose `contract()` lists `tool_timeout` but
//! whose `slow_spec` call returns `tool_exception`, When `run_contract`
//! runs, Then it fails with `kind=<name> case=budget_exceeded
//! code=tool_exception not in contract` and the other four cases still
//! report.

use async_trait::async_trait;
use mcphost::kinds::conformance::{CaseOutcome, run_contract, run_contract_report};
use mcphost::kinds::contract::{
    BudgetDim, CASE_BUDGET_EXCEEDED, CASE_ERROR_WITH_SOURCE_POSITION, CaseSkip, ConformanceFixtures,
    ErrorFamily, KindContract, SourcePosition, codes, family,
};
use mcphost::kinds::{CallCtx, Kind, KindError, ToolDescriptor};
use serde_json::{Value, json};

/// Answers `{"mode": "slow"}` specs with `tool_exception` -- a code its own
/// contract does not list.
struct DriftedKind;

static DRIFTED_CONTRACT: KindContract = KindContract {
    error_families: &[ErrorFamily {
        name: family::TOOL_LIMIT,
        codes: &[codes::TOOL_TIMEOUT],
    }],
    budgets: &[BudgetDim { field: "timeout_s", min: 1, max: 30, default: 5 }],
    source_position: SourcePosition::None,
    bridges: &[],
    secret_redaction: false,
    fixtures: ConformanceFixtures {
        good_spec: || json!({"mode": "ok"}),
        good_args: || json!({"msg": "hi"}),
        bad_args: || json!("not an object"),
        erroring_spec: None,
        slow_spec: Some(|| json!({"mode": "slow"})),
        secret_spec: None,
        skips: &[CaseSkip {
            case: CASE_ERROR_WITH_SOURCE_POSITION,
            reason: "drifted test kind has no failing fixture",
        }],
    },
};

#[async_trait]
impl Kind for DriftedKind {
    fn name(&self) -> &'static str {
        "drifted"
    }

    fn contract(&self) -> &'static KindContract {
        &DRIFTED_CONTRACT
    }

    fn validate(&self, _spec: &Value) -> Result<(), KindError> {
        Ok(())
    }

    fn describe(&self, _spec: &Value) -> ToolDescriptor {
        ToolDescriptor {
            name: "drifted".to_string(),
            description: "a kind whose slow call returns a code outside its contract".to_string(),
            input_schema: json!({"type": "object"}),
        }
    }

    async fn call(&self, spec: &Value, args: Value, _ctx: &CallCtx) -> Result<Value, KindError> {
        if !args.is_object() {
            return Err(KindError::InvalidArgs("args: must be an object".into()));
        }
        if spec["mode"] == "slow" {
            return Err(KindError::structured(codes::TOOL_EXCEPTION, "drifted"));
        }
        Ok(args)
    }
}

#[tokio::test]
async fn kcon_ac01_planted_drift_fails_naming_kind_case_code() {
    let failures = run_contract(&DriftedKind).await;
    assert_eq!(failures.len(), 1, "exactly the planted drift fails: {failures:?}");
    assert_eq!(
        failures[0].reason,
        "kind=drifted case=budget_exceeded code=tool_exception not in contract"
    );
    assert_eq!(failures[0].method, CASE_BUDGET_EXCEEDED);
}

#[tokio::test]
async fn kcon_ac01_the_other_four_cases_still_report() {
    let report = run_contract_report(&DriftedKind).await;
    let cases: Vec<&str> = report.iter().map(|r| r.case).collect();
    assert_eq!(
        cases,
        ["publish", "call", "error_with_source_position", "budget_exceeded", "secret_redaction"]
    );
    for r in &report {
        match (r.case, &r.outcome) {
            ("budget_exceeded", CaseOutcome::Fail(_)) => {}
            ("budget_exceeded", other) => panic!("budget_exceeded must fail, got {other:?}"),
            (_, CaseOutcome::Fail(reason)) => panic!("{} must not fail: {reason}", r.case),
            _ => {}
        }
    }
}
