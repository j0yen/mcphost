//! `KindContract` (PRD-mcphost-kind-contract-conformance-generated): the one
//! value every [`Kind`] publishes about what a call may return on failure --
//! error families and their codes, budget dimensions, source-position
//! capability, bridges, secret redaction -- plus the fixtures the generated
//! conformance suite ([`super::conformance::run_contract`]) drives it with.
//!
//! This file is the only place in `src/kinds/` that spells an error-code
//! string: every kind refers to the [`code`] constants below, so a code
//! cannot exist outside the table, and `family_of` is derived from the same
//! families a contract lists.

use serde_json::{Value, json};

use super::KindError;

/// Every error code and tagged key spelled in `src/kinds/`, once.
pub mod codes {
    pub const TOOL_EXCEPTION: &str = "tool_exception";
    pub const TOOL_TRAPPED: &str = "tool_trapped";
    pub const TOOL_OUTPUT_INVALID: &str = "tool_output_invalid";
    pub const TOOL_TIMEOUT: &str = "tool_timeout";
    pub const TOOL_OOM: &str = "tool_oom";
    pub const TOOL_PROCESS_LIMIT: &str = "tool_process_limit";
    pub const TOOL_OUTPUT_TOO_LARGE: &str = "tool_output_too_large";
    pub const TOOL_BUILDING: &str = "tool_building";
    pub const TOOL_NOT_FOUND: &str = "tool_not_found";
    pub const TOOL_RUN_UNSUPPORTED: &str = "tool_run_unsupported";
    pub const UPSTREAM_AUTH: &str = "upstream_auth";
    pub const UPSTREAM_RATE_LIMITED: &str = "upstream_rate_limited";
    pub const UPSTREAM_CLIENT_ERROR: &str = "upstream_client_error";
    pub const UPSTREAM_ERROR: &str = "upstream_error";
    pub const UPSTREAM_TIMEOUT: &str = "upstream_timeout";
    pub const UPSTREAM_UNREACHABLE: &str = "upstream_unreachable";
    pub const UPSTREAM_NOT_CONNECTED: &str = "upstream_not_connected";
    pub const HOST_NOT_ALLOWED: &str = "host_not_allowed";
    pub const COMPOSE_SELF_CALL: &str = "compose_self_call";
    pub const COMPOSE_DEPTH_EXCEEDED: &str = "compose_depth_exceeded";
    pub const COMPOSE_CHILDREN_EXCEEDED: &str = "compose_children_exceeded";
    pub const COMPOSE_ITEM_OUTSIDE_FOR_EACH: &str = "compose_item_outside_for_each";
    pub const COMPOSE_MAPPING_TYPE: &str = "compose_mapping_type";
    pub const COMPOSE_MAPPING_MISSING: &str = "compose_mapping_missing";
    pub const COMPOSE_FANOUT_EXCEEDED: &str = "compose_fanout_exceeded";
    pub const COMPOSE_FANOUT_ITEM_FAILED: &str = "compose_fanout_item_failed";
    pub const COMPOSE_INPUT_MISSING: &str = "compose_input_missing";
}

/// Non-code strings that share a code prefix (field names, phases).
pub mod key {
    pub const UPSTREAM_PROVIDER: &str = "upstream_provider";
    pub const UPSTREAM_MS: &str = "upstream_ms";
    pub const UPSTREAM_STATUS: &str = "upstream_status";
    pub const TOOL_CODE_MS: &str = "tool_code_ms";
    pub const PHASE_TOOL_CODE: &str = "tool_code";
}

/// Family names (`data.families` on the wire).
pub mod family {
    pub const TOOL_CODE: &str = "tool_code";
    pub const TOOL_LIMIT: &str = "tool_limit";
    pub const UPSTREAM: &str = "upstream";
    pub const COMPOSE: &str = "compose";
    pub const TOOL_STATE: &str = "tool_state";
}

#[derive(Debug, Clone, Copy)]
pub struct ErrorFamily {
    pub name: &'static str,
    pub codes: &'static [&'static str],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourcePosition {
    Line,
    LineColumn,
    Field(&'static str),
    None,
}

#[derive(Debug, Clone, Copy)]
pub struct BudgetDim {
    pub field: &'static str,
    pub min: u64,
    pub max: u64,
    pub default: u64,
}

/// A conformance case a kind does not run, and the reason its contract
/// states for that.
#[derive(Debug, Clone, Copy)]
pub struct CaseSkip {
    pub case: &'static str,
    pub reason: &'static str,
}

/// The inputs `run_contract` drives a kind with. Fixtures are functions so
/// a contract can be a `static`.
#[derive(Clone, Copy)]
pub struct ConformanceFixtures {
    pub good_spec: fn() -> Value,
    pub good_args: fn() -> Value,
    pub bad_args: fn() -> Value,
    /// A spec whose call fails with a `tool_code` family code.
    pub erroring_spec: Option<fn() -> Value>,
    /// A spec whose call outruns the smallest `timeout_s` budget.
    pub slow_spec: Option<fn() -> Value>,
    /// A spec whose call returns the value of the conformance secret
    /// (`SECRET_NAME`) in its result.
    pub secret_spec: Option<fn() -> Value>,
    pub skips: &'static [CaseSkip],
}

impl ConformanceFixtures {
    pub fn skip_reason(&self, case: &str) -> Option<&'static str> {
        self.skips.iter().find(|s| s.case == case).map(|s| s.reason)
    }
}

/// What a kind may return on failure, and how the suite drives it.
pub struct KindContract {
    pub error_families: &'static [ErrorFamily],
    pub budgets: &'static [BudgetDim],
    pub source_position: SourcePosition,
    pub bridges: &'static [&'static str],
    pub secret_redaction: bool,
    pub fixtures: ConformanceFixtures,
}

impl KindContract {
    pub fn family_of(&self, code: &str) -> Option<&'static str> {
        self.error_families
            .iter()
            .find(|f| f.codes.contains(&code))
            .map(|f| f.name)
    }

    pub fn has_code(&self, code: &str) -> bool {
        self.family_of(code).is_some()
    }
}

pub const CASE_PUBLISH: &str = "publish";
pub const CASE_CALL: &str = "call";
pub const CASE_ERROR_WITH_SOURCE_POSITION: &str = "error_with_source_position";
pub const CASE_BUDGET_EXCEEDED: &str = "budget_exceeded";
pub const CASE_SECRET_REDACTION: &str = "secret_redaction";

/// The conformance secret `run_contract` resolves: `secret_spec` fixtures
/// declare it by [`CONFORMANCE_SECRET_NAME`] and must not leak the value.
pub const CONFORMANCE_SECRET_NAME: &str = "conformance_secret";
pub const CONFORMANCE_SECRET_VALUE: &str = "sk-conformance-canary-7f3a91";

pub const TOOL_CODE_FAMILY: ErrorFamily = ErrorFamily {
    name: family::TOOL_CODE,
    codes: &[codes::TOOL_EXCEPTION, codes::TOOL_TRAPPED, codes::TOOL_OUTPUT_INVALID],
};
pub const TOOL_LIMIT_FAMILY: ErrorFamily = ErrorFamily {
    name: family::TOOL_LIMIT,
    codes: &[
        codes::TOOL_TIMEOUT,
        codes::TOOL_OOM,
        codes::TOOL_PROCESS_LIMIT,
        codes::TOOL_OUTPUT_TOO_LARGE,
    ],
};
pub const UPSTREAM_FAMILY: ErrorFamily = ErrorFamily {
    name: family::UPSTREAM,
    codes: &[
        codes::UPSTREAM_AUTH,
        codes::UPSTREAM_RATE_LIMITED,
        codes::UPSTREAM_CLIENT_ERROR,
        codes::UPSTREAM_ERROR,
        codes::UPSTREAM_TIMEOUT,
        codes::UPSTREAM_UNREACHABLE,
        codes::UPSTREAM_NOT_CONNECTED,
        codes::HOST_NOT_ALLOWED,
    ],
};
pub const COMPOSE_FAMILY: ErrorFamily = ErrorFamily {
    name: family::COMPOSE,
    codes: &[
        codes::COMPOSE_SELF_CALL,
        codes::COMPOSE_DEPTH_EXCEEDED,
        codes::COMPOSE_CHILDREN_EXCEEDED,
        codes::COMPOSE_ITEM_OUTSIDE_FOR_EACH,
        codes::COMPOSE_MAPPING_TYPE,
        codes::COMPOSE_MAPPING_MISSING,
        codes::COMPOSE_FANOUT_EXCEEDED,
        codes::COMPOSE_FANOUT_ITEM_FAILED,
        codes::COMPOSE_INPUT_MISSING,
    ],
};
pub const TOOL_STATE_FAMILY: ErrorFamily = ErrorFamily {
    name: family::TOOL_STATE,
    codes: &[codes::TOOL_BUILDING, codes::TOOL_NOT_FOUND, codes::TOOL_RUN_UNSUPPORTED],
};

/// Every family the host knows, the superset each kind's contract lists a
/// subset of.
pub fn all_families() -> [&'static ErrorFamily; 5] {
    [
        &TOOL_CODE_FAMILY,
        &TOOL_LIMIT_FAMILY,
        &UPSTREAM_FAMILY,
        &COMPOSE_FAMILY,
        &TOOL_STATE_FAMILY,
    ]
}

/// The family a code belongs to, across the whole host.
pub fn family_of(code: &str) -> Option<&'static str> {
    all_families()
        .into_iter()
        .find(|f| f.codes.contains(&code))
        .map(|f| f.name)
}

/// Tags a sandboxed-tool error (`tool_code` / `tool_limit` family) with
/// `data.families: [<family>]`, additively -- every other error is returned
/// unchanged.
pub fn tag_families(err: KindError) -> KindError {
    match err {
        KindError::Structured { code, message, data } => {
            let tagged = matches!(
                family_of(code),
                Some(family::TOOL_CODE) | Some(family::TOOL_LIMIT)
            );
            let data = match (tagged, data) {
                (true, Value::Null) => json!({"families": [family_of(code)]}),
                (true, Value::Object(mut map)) => {
                    map.entry("families").or_insert_with(|| json!([family_of(code)]));
                    Value::Object(map)
                }
                (_, data) => data,
            };
            KindError::Structured { code, message, data }
        }
        other => other,
    }
}

fn empty_object() -> Value {
    json!({})
}

fn not_an_object() -> Value {
    json!("not an object")
}

/// A contract for a kind that declares no failure families, budgets or
/// bridges (test-only kinds): every non-publish case is skipped with a
/// stated reason.
pub static EMPTY_CONTRACT: KindContract = KindContract {
    error_families: &[],
    budgets: &[],
    source_position: SourcePosition::None,
    bridges: &[],
    secret_redaction: false,
    fixtures: ConformanceFixtures {
        good_spec: empty_object,
        good_args: empty_object,
        bad_args: not_an_object,
        erroring_spec: None,
        slow_spec: None,
        secret_spec: None,
        skips: &[
            CaseSkip { case: CASE_CALL, reason: "test-only kind: no call fixture" },
            CaseSkip { case: CASE_ERROR_WITH_SOURCE_POSITION, reason: "test-only kind: no failing fixture" },
            CaseSkip { case: CASE_BUDGET_EXCEEDED, reason: "test-only kind: no budget" },
        ],
    },
};
