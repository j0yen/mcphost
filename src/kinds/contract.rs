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
    pub const JQ_BUILTIN_UNSUPPORTED: &str = "jq_builtin_unsupported";
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
    /// `UPSTREAM_REMEDIES` row keys that refine `host_not_allowed` (dotted, so not registry codes).
    pub const OWN_HOOK_EVENT: &str = "host_not_allowed.own_hook_event";
    pub const OWN_HOOK_INBOX: &str = "host_not_allowed.own_hook_inbox";
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

pub const CONTRACT_START: &str = "<!-- kind-contract:start -->";
pub const CONTRACT_END: &str = "<!-- kind-contract:end -->";

impl SourcePosition {
    /// The wire word for this capability (`contracts/host-tools.v1.json`
    /// and the docs table).
    pub fn as_word(&self) -> String {
        match self {
            SourcePosition::Line => "line".to_string(),
            SourcePosition::LineColumn => "line_column".to_string(),
            SourcePosition::Field(f) => format!("field:{f}"),
            SourcePosition::None => "none".to_string(),
        }
    }
}

impl KindContract {
    /// `families: tool_code, tool_limit; source_line: yes` -- the one-line
    /// summary `host.tool_publish`'s kind description and
    /// `host.quickstart`/`host.spec_test` carry.
    pub fn summary_line(&self) -> String {
        let names: Vec<&str> = self.error_families.iter().map(|f| f.name).collect();
        format!(
            "families: {}; source_line: {}",
            if names.is_empty() { "none".to_string() } else { names.join(", ") },
            if self.source_position == SourcePosition::Line { "yes" } else { "no" }
        )
    }

    /// `{families, source_line}` for `host.quickstart`/`host.spec_test`.
    pub fn response_block(&self) -> Value {
        json!({
            "families": self.error_families.iter().map(|f| f.name).collect::<Vec<_>>(),
            "source_line": self.source_position == SourcePosition::Line,
        })
    }

    /// The contract as `contracts/host-tools.v1.json` carries it: fixtures
    /// omitted.
    pub fn contract_json(&self) -> Value {
        json!({
            "error_families": self.error_families.iter()
                .map(|f| json!({"name": f.name, "codes": f.codes}))
                .collect::<Vec<_>>(),
            "budgets": self.budgets.iter()
                .map(|b| json!({"field": b.field, "min": b.min, "max": b.max, "default": b.default}))
                .collect::<Vec<_>>(),
            "source_position": self.source_position.as_word(),
            "bridges": self.bridges,
            "secret_redaction": self.secret_redaction,
        })
    }

    /// The table rows of the `## Contract` block, one per fact, so a
    /// drift check can name the row.
    pub fn doc_rows(&self) -> Vec<String> {
        let mut rows = Vec::new();
        for b in self.budgets {
            rows.push(format!("| budget `{}` | {}-{} (default {}) |", b.field, b.min, b.max, b.default));
        }
        for f in self.error_families {
            rows.push(format!("| family `{}` | {} |", f.name, f.codes.join(", ")));
        }
        let pos = match self.source_position {
            SourcePosition::Line => "yes (`data.source_line`)".to_string(),
            SourcePosition::LineColumn => "yes (line and column)".to_string(),
            SourcePosition::Field(f) => format!("field `{f}`"),
            SourcePosition::None => "no".to_string(),
        };
        rows.push(format!("| source_line | {pos} |"));
        rows.push(format!(
            "| bridges | {} |",
            if self.bridges.is_empty() { "none".to_string() } else { self.bridges.join(", ") }
        ));
        let redaction = if self.secret_redaction {
            "yes".to_string()
        } else {
            match self.fixtures.skip_reason(CASE_SECRET_REDACTION) {
                Some(r) => format!("no (skipped: {r})"),
                None => "no".to_string(),
            }
        };
        rows.push(format!("| secret redaction | {redaction} |"));
        rows
    }

    /// The generated `## Contract` section, markers included.
    pub fn render_doc_block(&self) -> String {
        let mut out = format!("{CONTRACT_START}\n## Contract\n\n| Field | Value |\n|---|---|\n");
        for row in self.doc_rows() {
            out.push_str(&row);
            out.push('\n');
        }
        out.push_str(CONTRACT_END);
        out.push('\n');
        out
    }
}

/// Replaces the marked block in `doc` with `block`, or appends it when the
/// page has no markers yet.
pub fn splice_doc_block(doc: &str, block: &str) -> String {
    if let (Some(start), Some(end)) = (doc.find(CONTRACT_START), doc.find(CONTRACT_END))
        && start < end
    {
        let after = &doc[end + CONTRACT_END.len()..];
        let after = after.strip_prefix('\n').unwrap_or(after);
        return format!("{}{}{}", &doc[..start], block, after);
    }
    let mut out = doc.trim_end_matches('\n').to_string();
    out.push_str("\n\n");
    out.push_str(block);
    out
}

crate::err_code! {
    pub COMPOSE_CHILDREN_EXCEEDED,
    code: "compose_children_exceeded", family: Compose, class: AgentFault, retryable: false,
    remedy: "Reduce the number of child calls in the compose spec to the plan ceiling.",
}

crate::err_code! {
    pub COMPOSE_DEPTH_EXCEEDED,
    code: "compose_depth_exceeded", family: Compose, class: AgentFault, retryable: false,
    remedy: "Flatten the compose spec; nesting is deeper than the allowed depth.",
}

crate::err_code! {
    pub COMPOSE_FANOUT_EXCEEDED,
    code: "compose_fanout_exceeded", family: Compose, class: AgentFault, retryable: false,
    remedy: "Reduce the for_each fan-out to the allowed number of items.",
}

crate::err_code! {
    pub COMPOSE_FANOUT_ITEM_FAILED,
    code: "compose_fanout_item_failed", family: Compose, class: UpstreamFault, retryable: false,
    remedy: "Read data for the failing item's own error and fix or skip that item.",
}

crate::err_code! {
    pub COMPOSE_INPUT_MISSING,
    code: "compose_input_missing", family: Compose, class: AgentFault, retryable: false,
    remedy: "Pass the input the compose mapping reads, named in the message.",
}

crate::err_code! {
    pub COMPOSE_ITEM_OUTSIDE_FOR_EACH,
    code: "compose_item_outside_for_each", family: Compose, class: AgentFault, retryable: false,
    remedy: "Use item only inside a for_each mapping.",
}

crate::err_code! {
    pub COMPOSE_MAPPING_MISSING,
    code: "compose_mapping_missing", family: Compose, class: AgentFault, retryable: false,
    remedy: "Add the missing mapping for the child argument named in the message.",
}

crate::err_code! {
    pub COMPOSE_MAPPING_TYPE,
    code: "compose_mapping_type", family: Compose, class: AgentFault, retryable: false,
    remedy: "Map a value of the child argument's declared type.",
}

crate::err_code! {
    pub COMPOSE_SELF_CALL,
    code: "compose_self_call", family: Compose, class: AgentFault, retryable: false,
    remedy: "A compose tool may not call itself; point the step at a different tool.",
}

crate::err_code! {
    pub TOOL_BUILDING,
    code: "tool_building", family: Call, class: HostFault, retryable: true,
    remedy: "Retry shortly; the tool's environment is still building.",
}

crate::err_code! {
    pub TOOL_OOM,
    code: "tool_oom", family: Call, class: AgentFault, retryable: false,
    remedy: "Reduce the tool's memory use or the input size.",
}

crate::err_code! {
    pub TOOL_OUTPUT_INVALID,
    code: "tool_output_invalid", family: Call, class: AgentFault, retryable: false,
    remedy: "Return JSON that matches the tool's declared output.",
}

crate::err_code! {
    pub TOOL_OUTPUT_TOO_LARGE,
    code: "tool_output_too_large", family: Call, class: AgentFault, retryable: false,
    remedy: "Return less data; the output exceeds the cap.",
}

crate::err_code! {
    pub TOOL_PROCESS_LIMIT,
    code: "tool_process_limit", family: Call, class: AgentFault, retryable: false,
    remedy: "Reduce the number of processes or threads the tool starts.",
}

crate::err_code! {
    pub TOOL_RUN_UNSUPPORTED,
    code: "tool_run_unsupported", family: Call, class: HostRefusal, retryable: false,
    remedy: "This kind cannot be run this way; call the tool through host.tool_call.",
}

crate::err_code! {
    pub TOOL_TRAPPED,
    code: "tool_trapped", family: Call, class: AgentFault, retryable: false,
    remedy: "The tool trapped; fix the runtime fault in its code and republish.",
}

crate::err_code! {
    pub UPSTREAM_AUTH,
    code: "upstream_auth", family: Upstream, class: UpstreamFault, retryable: false,
    remedy: "Reconnect the upstream or fix the secret the tool sends; the upstream rejected the credential.",
}

crate::err_code! {
    pub UPSTREAM_CLIENT_ERROR,
    code: "upstream_client_error", family: Upstream, class: UpstreamFault, retryable: false,
    remedy: "Fix the request the tool sends; the upstream rejected it as a client error.",
}

crate::err_code! {
    pub UPSTREAM_ERROR,
    code: "upstream_error", family: Upstream, class: UpstreamFault, retryable: true,
    remedy: "Retry later; the upstream failed with a server error.",
}

crate::err_code! {
    pub UPSTREAM_RATE_LIMITED,
    code: "upstream_rate_limited", family: Upstream, class: UpstreamFault, retryable: true,
    remedy: "Wait and retry; the upstream is rate limiting this tool.",
}

crate::err_code! {
    pub UPSTREAM_STATUS,
    code: "upstream_status", family: Upstream, class: UpstreamFault, retryable: false,
    remedy: "Read data.upstream_status for the upstream's HTTP status, then fix the request or retry later.",
}

crate::err_code! {
    pub UPSTREAM_TIMEOUT,
    code: "upstream_timeout", family: Upstream, class: UpstreamFault, retryable: true,
    remedy: "Retry later; the upstream did not answer in time.",
}

crate::err_code! {
    pub UPSTREAM_UNREACHABLE,
    code: "upstream_unreachable", family: Upstream, class: UpstreamFault, retryable: true,
    remedy: "Retry later; the upstream could not be reached.",
}

