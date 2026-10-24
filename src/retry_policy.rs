//! PRD-mcphost-retry-policy-table-generated: the one table that answers "is
//! this error worth retrying, how soon, how many times". Every JSON-RPC error
//! the host renders carries `data.retry {retryable, after_ms, attempts_left,
//! policy}` computed from [`RETRY_POLICY`]; `www/llms.txt`, `docs/errors.md`
//! and `www/retry_policy.json` are rendered from the same rows by
//! `mcphost retry-policy`, and the chain step's `retry.on` is validated
//! against [`retryable_codes`]. The host itself never retries anything.

use serde_json::{Map, Value, json};

/// How long to wait before attempt `n + 1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backoff {
    None,
    Fixed(u64),
    Exponential { base_ms: u64, max_ms: u64 },
}

impl Backoff {
    /// The wait after failed attempt `attempt` (1-based): `min(base * 2^(n-1), max)`.
    pub fn after_ms(&self, attempt: u32) -> Option<u64> {
        match *self {
            Backoff::None => None,
            Backoff::Fixed(ms) => Some(ms),
            Backoff::Exponential { base_ms, max_ms } => {
                let shift = attempt.saturating_sub(1).min(32);
                Some(base_ms.saturating_mul(1u64 << shift).min(max_ms))
            }
        }
    }

    fn to_json(self) -> Value {
        match self {
            Backoff::None => json!({"kind": "none"}),
            Backoff::Fixed(ms) => json!({"kind": "fixed", "ms": ms}),
            Backoff::Exponential { base_ms, max_ms } => {
                json!({"kind": "exponential", "base_ms": base_ms, "max_ms": max_ms})
            }
        }
    }

    fn describe(self) -> String {
        match self {
            Backoff::None => "none".to_string(),
            Backoff::Fixed(ms) => format!("fixed({ms}ms)"),
            Backoff::Exponential { base_ms, max_ms } => format!("exponential({base_ms}ms, max {max_ms}ms)"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryRow {
    pub code: &'static str,
    pub retryable: bool,
    pub backoff: Backoff,
    pub max_attempts: u8,
    pub policy: &'static str,
}

const FIX: &str = "fix-and-resend";
const BACKOFF: &str = "backoff-then-retry";
const AFTER: &str = "honour-retry-after";
const IDEM: &str = "retry-with-idempotency-key";
const WAIT: &str = "wait-then-retry";
const AUTH: &str = "reauthenticate";
const NO: &str = "do-not-retry";

const fn no(code: &'static str, policy: &'static str) -> RetryRow {
    RetryRow { code, retryable: false, backoff: Backoff::None, max_attempts: 1, policy }
}

const fn yes(code: &'static str, backoff: Backoff, max_attempts: u8, policy: &'static str) -> RetryRow {
    RetryRow { code, retryable: true, backoff, max_attempts, policy }
}

const fn exp(base_ms: u64, max_ms: u64) -> Backoff {
    Backoff::Exponential { base_ms, max_ms }
}

/// What an unknown code resolves to: not retryable, so an unlisted error
/// never invites a loop. [`policy_for`] logs a warn so the gap is visible.
pub const DEFAULT_UNKNOWN: RetryRow = no("unknown", NO);

/// One row per error code the host can emit (the completeness test in
/// `tests/rpol_ac02_*.rs` fails the build in both directions). Rows marked
/// retryable are the only codes a chain step's `retry.on` may name.
pub const RETRY_POLICY: &[RetryRow] = &[
    // AppError variants
    no("bearer_invalid", AUTH),
    no("tenant_key_missing", FIX),
    no("tenant_key_invalid", AUTH),
    no("forbidden", NO),
    no("tenant_disabled", NO),
    yes("rate_limited", exp(1000, 30_000), 3, AFTER),
    no("tool_not_found", NO),
    no("tenant_not_found", NO),
    no("invalid_params", FIX),
    no("unknown_kind", FIX),
    no("invalid_tool_name", FIX),
    no("spec_too_large", FIX),
    no("too_many_tools", NO),
    no("invalid_spec", FIX),
    no("args_invalid", FIX),
    no("secret_missing", FIX),
    yes("call_timeout", Backoff::Fixed(1000), 2, IDEM),
    yes("storage", exp(500, 8000), 2, BACKOFF),
    yes("internal", exp(500, 8000), 2, BACKOFF),
    no("registry_disabled", NO),
    no("namespace_unverified", NO),
    no("registry_rejected", NO),
    no("share_quota_exceeded", NO),
    no(crate::sharing::SCOPE_MISSING, NO),
    no(crate::sharing::SHARE_REVOKED, NO),
    no("group_not_found", NO),
    no("handoff_token_invalid", NO),
    no("handoff_token_redeemed", NO),
    no("handoff_token_expired", NO),
    no("issuer_already_registered", NO),
    no("issuer_quota_exceeded", NO),
    no("issuer_not_found", NO),
    no("invalid_token", AUTH),
    no("conflicting_credentials", FIX),
    no("wrong_tenant", NO),
    no(crate::scopes::INSUFFICIENT_SCOPE, NO),
    no("auth_conflict", FIX),
    no("quota_trusted_issuers", NO),
    // kind-level codes (`KIND_LEVEL_CODES` and `KindError::structured` literals)
    no("upstream_status", NO),
    no("unknown_argument", FIX),
    no("end_user_required", NO),
    no("end_user_explicit_forbidden", NO),
    no("args_schema_not_inferable", NO),
    no("budget_exceeded", NO),
    no("build_failed", NO),
    yes("capacity", exp(1000, 30_000), 3, BACKOFF),
    yes("channel_unavailable", Backoff::Fixed(5000), 3, BACKOFF),
    no(crate::chart::CHART_INVALID_MARK, FIX),
    no("compose_children_exceeded", NO),
    no("compose_depth_exceeded", NO),
    no("compose_fanout_exceeded", NO),
    no("compose_fanout_item_failed", NO),
    no("compose_input_missing", FIX),
    no("compose_item_outside_for_each", FIX),
    no("compose_mapping_missing", FIX),
    no("compose_mapping_type", FIX),
    no("compose_self_call", NO),
    yes("concurrency_exceeded", Backoff::Fixed(1000), 3, WAIT),
    yes("docs_unavailable", Backoff::Fixed(5000), 3, BACKOFF),
    yes("drift_unavailable", Backoff::Fixed(5000), 3, BACKOFF),
    yes("egress_unavailable", Backoff::Fixed(5000), 3, BACKOFF),
    no("elicit_calls_exceeded", NO),
    no("elicit_schema_invalid", FIX),
    no("elicit_timeout", NO),
    no("elicit_unsupported", NO),
    no("exec", NO),
    no("fixture_shape_mismatch", FIX),
    no("fixture_step_ambiguous", FIX),
    no("fixture_step_unknown", FIX),
    no(crate::tables::HANDLE_NOT_FOUND, FIX),
    no(crate::tables::HANDLE_QUOTA_EXCEEDED, NO),
    no("host_not_allowed", NO),
    no("jq_builtin_unsupported", NO),
    no("leak_test_failed", NO),
    no(crate::tables::LINEAGE_BLOCKED, FIX),
    yes("lineage_unavailable", Backoff::Fixed(5000), 3, BACKOFF),
    yes("msg_unavailable", Backoff::Fixed(5000), 3, BACKOFF),
    no(crate::subkeys::NOT_FOUND, FIX),
    no("plan_required", NO),
    no(crate::errors::QUOTA_EXCEEDED, NO),
    no("requirement_not_allowed", NO),
    no("requirement_not_inferable", NO),
    no("response_too_large", NO),
    no("sampling_calls_exceeded", NO),
    no("sampling_quota_exceeded", NO),
    yes("sampling_timeout", Backoff::Fixed(1000), 2, FIX),
    no("sampling_unsupported", NO),
    no("secret_leak_blocked", FIX),
    no("state_quota_exceeded", NO),
    yes("state_unavailable", Backoff::Fixed(5000), 3, BACKOFF),
    no(crate::tables::TABLE_ALREADY_EXISTS, FIX),
    no(crate::tables::TABLE_BOUND_EXCEEDED, FIX),
    no(crate::tables::TABLE_NOT_FOUND, FIX),
    no(crate::tables::TABLE_QUERY_REJECTED, FIX),
    no(crate::tables::TABLE_ROW_CONFLICT, FIX),
    yes("table_unavailable", Backoff::Fixed(5000), 3, BACKOFF),
    no("template_error", FIX),
    no("tool_exception", NO),
    no("tool_oom", NO),
    no("tool_output_invalid", NO),
    no("tool_output_too_large", NO),
    no("tool_process_limit", NO),
    no("tool_run_unsupported", NO),
    yes("tool_timeout", Backoff::Fixed(1000), 2, IDEM),
    no("tool_trapped", NO),
    no("typescript_not_supported", NO),
    no("unknown_import", FIX),
    no("unknown_spec_field", FIX),
    yes("uptime_unavailable", Backoff::Fixed(5000), 3, BACKOFF),
    // the `http` kind's UPSTREAM_REMEDIES families
    no("upstream_auth", NO),
    yes("upstream_rate_limited", exp(1000, 60_000), 3, AFTER),
    no("upstream_client_error", NO),
    yes("upstream_error", exp(1000, 30_000), 3, BACKOFF),
    yes("upstream_timeout", exp(1000, 30_000), 2, IDEM),
    yes("upstream_unreachable", exp(1000, 30_000), 3, BACKOFF),
    no("upstream_not_connected", NO),
    // handler-level `AppError::Structured` codes a caller may wait out
    yes("sandbox_unavailable", Backoff::Fixed(5000), 3, AFTER),
    yes("signup_paused", Backoff::Fixed(60_000), 3, AFTER),
    yes("signup_rate_limited", exp(1000, 60_000), 3, AFTER),
    yes("service_unavailable", exp(1000, 30_000), 3, BACKOFF),
    yes("tool_building", Backoff::Fixed(2000), 5, WAIT),
    yes("events_rate_limited", exp(1000, 30_000), 3, AFTER),
    yes("rate_limited_host", exp(1000, 30_000), 3, AFTER),
    yes("invite_rate_limited", exp(1000, 60_000), 3, AFTER),
];

/// The row for `code`; an unknown code resolves to [`DEFAULT_UNKNOWN`] and
/// logs a warn naming it.
pub fn policy_for(code: &str) -> &'static RetryRow {
    match RETRY_POLICY.iter().find(|r| r.code == code) {
        Some(row) => row,
        None => {
            tracing::warn!(code = %code, "retry_policy: no RETRY_POLICY row for this error code");
            &DEFAULT_UNKNOWN
        }
    }
}

/// The codes a chain step's `retry.on` may name, in table order.
pub fn retryable_codes() -> Vec<&'static str> {
    RETRY_POLICY.iter().filter(|r| r.retryable).map(|r| r.code).collect()
}

/// Whether `code` has a row and that row is retryable. Unlike [`policy_for`],
/// an unknown code is simply `false` (no warn: it is a caller's typo).
pub fn is_retryable_code(code: &str) -> bool {
    RETRY_POLICY.iter().any(|r| r.code == code && r.retryable)
}

/// The 1-based attempt number a caller reports in `_meta.attempt` (default 1).
pub fn attempt_from_meta(meta: Option<&Map<String, Value>>) -> u32 {
    meta.and_then(|m| m.get("attempt"))
        .and_then(Value::as_u64)
        .map(|n| n.clamp(1, u32::MAX as u64) as u32)
        .unwrap_or(1)
}

/// The `data.retry` block for `code` on attempt `attempt`. `retry_after_s` is
/// the upstream's own wait, when the error carries one, and wins over the
/// row's backoff.
pub fn retry_block(code: &str, retry_after_s: Option<u64>, attempt: u32) -> Value {
    let row = policy_for(code);
    let attempts_left = u32::from(row.max_attempts).saturating_sub(attempt);
    let retryable = row.retryable && attempts_left > 0;
    let after_ms = if retryable {
        retry_after_s.map(|s| s.saturating_mul(1000)).or_else(|| row.backoff.after_ms(attempt))
    } else {
        None
    };
    json!({
        "retryable": retryable,
        "after_ms": after_ms,
        "attempts_left": attempts_left,
        "policy": row.policy,
    })
}

/// Adds `retry` to an error payload object, reading `retry_after_s` (or the
/// signup path's `retry_after_secs`) from the payload itself.
pub fn attach(obj: &mut Map<String, Value>, code: &str, attempt: u32) {
    let retry_after_s = ["retry_after_s", "retry_after_secs"]
        .iter()
        .find_map(|k| obj.get(*k))
        .and_then(Value::as_u64);
    obj.insert("retry".to_string(), retry_block(code, retry_after_s, attempt));
}

pub const SECTION_START: &str = "<!-- retry-policy:start -->";
pub const SECTION_END: &str = "<!-- retry-policy:end -->";

fn render_table() -> String {
    let mut out = String::from("| code | retryable | backoff | max attempts | policy |\n|---|---|---|---|---|\n");
    for r in RETRY_POLICY {
        out.push_str(&format!(
            "| `{}` | {} | {} | {} | {} |\n",
            r.code,
            if r.retryable { "yes" } else { "no" },
            r.backoff.describe(),
            r.max_attempts,
            r.policy
        ));
    }
    out
}

/// The `## Retry policy` section of `www/llms.txt`, markers included. Table
/// rows, not `- ` bullets: the tools parity test reads bullets as tool names.
pub fn render_section() -> String {
    format!(
        "{SECTION_START}\n## Retry policy\n\n\
         Generated by `mcphost retry-policy` from `RETRY_POLICY` -- the same rows every error's \
         `data.retry {{retryable, after_ms, attempts_left, policy}}` is built from; do not hand-edit. \
         Sleep `after_ms` before a retry; send `_meta.attempt` so `attempts_left` counts down. \
         Machine-readable: `/retry_policy.json`.\n\n{}{SECTION_END}",
        render_table()
    )
}

/// The whole of `docs/errors.md`.
pub fn render_doc() -> String {
    format!("# Error retry policy\n\n{}\n\n{}\n", render_section(), render_data_keys_section())
}

/// The `## Error data keys` section of `docs/errors.md`: one row per registry
/// code that declares `data_keys` (`ErrorEntry::data_keys`), generated.
pub fn render_data_keys_section() -> String {
    let mut out = String::from(
        "## Error data keys\n\nGenerated from the error registry's `data_keys`; do not hand-edit. \
         Keys a code's `data` carries beyond the shared envelope ones.\n\n| code | data keys |\n|---|---|\n",
    );
    for e in crate::error_registry::ERROR_CODES.iter().filter(|e| !e.data_keys.is_empty()) {
        let keys: Vec<String> = e.data_keys.iter().map(|k| format!("`{k}`")).collect();
        out.push_str(&format!("| `{}` | {} |\n", e.code, keys.join(", ")));
    }
    out
}

/// `www/retry_policy.json`.
pub fn render_json() -> String {
    let rows: Vec<Value> = RETRY_POLICY
        .iter()
        .map(|r| {
            json!({
                "code": r.code,
                "retryable": r.retryable,
                "backoff": r.backoff.to_json(),
                "max_attempts": r.max_attempts,
                "policy": r.policy,
            })
        })
        .collect();
    let mut s = serde_json::to_string_pretty(&json!({"version": 1, "rows": rows})).unwrap_or_default();
    s.push('\n');
    s
}

/// The marked retry-policy block of `content` (markers included), if both exist.
pub fn section_block(content: &str) -> Option<&str> {
    let start = content.find(SECTION_START)?;
    let end = content.find(SECTION_END)? + SECTION_END.len();
    content.get(start..end)
}

/// Replace the marked block in `content`, or -- before any marker exists --
/// insert it just before `## Full documentation` (else append).
pub fn splice_into(content: &str, section: &str) -> String {
    let section = section.trim_end();
    if let (Some(start), Some(end_pos)) = (content.find(SECTION_START), content.find(SECTION_END)) {
        let end = end_pos + SECTION_END.len();
        format!("{}{}{}", &content[..start], section, &content[end..])
    } else if let Some(idx) = content.find("## Full documentation") {
        format!("{}{}\n\n{}", &content[..idx], section, &content[idx..])
    } else {
        format!("{}\n\n{}\n", content.trim_end(), section)
    }
}
