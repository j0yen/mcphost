//! `mcphost`'s own error taxonomy, and its mapping onto MCP JSON-RPC errors.
//!
//! Every acceptance criterion that names an error code (`tenant_disabled`,
//! `tool_not_found`, `rate_limited`, ...) gets it from [`AppError::code`],
//! carried in the JSON-RPC error's `data.error_code` field so a caller (or a
//! test) can match on it without parsing prose.

use rmcp::model::{ErrorCode, ErrorData};
use serde_json::{Map, Value, json};

use crate::kinds::KindError;
use crate::kinds::infer::KindSignal;

/// A corrected example value for a well-known spec/argument field name,
/// shared by every kind's rejections (PRD-mcphost-publish-first-try
/// requirement 2, AC2). Keyed by the bare field name a `KindError`'s
/// "<field>: <description>" message convention already exposes (see
/// [`AppError::split_field`]) -- adding a kind's own field here is the only
/// step needed for its rejections to start carrying a real, resubmittable
/// `example`, no call-site changes required.
fn field_example(field: &str) -> Option<Value> {
    Some(match field {
        "name" => json!("my_tool"),
        "kind" => json!("echo"),
        "spec.schema" | "schema" => json!({
            "type": "object",
            "properties": {"msg": {"type": "string"}},
            "required": ["msg"],
        }),
        "method" => json!("GET"),
        "url" => json!("https://api.example.com/items/{{id}}"),
        "timeout_s" => json!(10),
        "response" => json!("json"),
        "body" => json!({"type": "object"}),
        "args_schema" => json!({"type": "object", "properties": {}}),
        "source" => json!("def main(args):\n    return {\"ok\": True}\n"),
        "requirements" => json!(["requests"]),
        "memory_mb" => json!(256),
        "network" => json!("none"),
        _ => return None,
    })
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum AppError {
    #[error("missing or invalid Authorization: Bearer key")]
    Unauthorized,
    /// PRD-mcphost-auth-error-names-argument requirement 1 / AC1: a
    /// `host.*` call with no `Authorization` header and no `tenant_key`
    /// argument (or a non-string one) -- the argument-only auth path
    /// (mcphost-session-key) has no header to be "missing or invalid", so
    /// this names the argument the caller can actually pass instead of
    /// reusing [`AppError::Unauthorized`]'s header-shaped text.
    #[error("tenant_key is required: pass the key that signup returned as the tenant_key argument")]
    TenantKeyMissing,
    /// PRD-mcphost-auth-error-names-argument requirement 2 / AC2-3: a
    /// `tenant_key` argument present but matching no tenant (or a disabled
    /// one, which stays [`AppError::TenantDisabled`] -- unchanged, see
    /// `resolve_tenant_key_auth`). Never interpolates the offending key.
    #[error("tenant_key was not recognized; call signup for a new key or check the value")]
    TenantKeyInvalid,
    #[error("forbidden")]
    Forbidden,
    #[error("tenant is disabled")]
    TenantDisabled,
    #[error("signup rate limit exceeded for this source; try again later")]
    RateLimited,
    #[error("tool not found: {0}")]
    ToolNotFound(String),
    /// PRD-mcphost-tenant-delete AC7: distinct from [`AppError::ToolNotFound`]
    /// so `admin.tenant_delete` on an unknown tenant reads as
    /// `tenant_not_found`, not `tool_not_found`.
    #[error("tenant not found: {0}")]
    TenantNotFound(String),
    /// PRD-mcphost-tenant-delete AC5: `admin.tenant_delete_by_prefix`'s
    /// `prefix` must be at least 4 characters, so a typo (or an omitted
    /// argument) cannot empty the box. Distinct from
    /// [`AppError::InvalidArgs`] because the PRD pins the wire code to
    /// exactly `invalid_params`.
    #[error("invalid params: {0}")]
    InvalidParams(String),
    #[error("kind '{requested}' is not registered; registered kinds: {registered:?}")]
    UnknownKind {
        requested: String,
        registered: Vec<&'static str>,
    },
    #[error("invalid tool name '{0}': must match ^[a-z][a-z0-9_]{{1,40}}$")]
    InvalidToolName(String),
    #[error("spec is {0} bytes, over the 64 KiB limit")]
    SpecTooLarge(usize),
    #[error("tenant already holds {0} tools, the maximum")]
    TooManyTools(usize),
    #[error("invalid spec: {0}")]
    InvalidSpec(String),
    /// PRD-mcphost-surface-fluidity requirement 2 (Goal 2): the host used to
    /// carry two wire codes for one mistake -- this variant's own (now
    /// retired) code and the schema pre-check's `args_invalid` below. Both
    /// now emit `args_invalid` (see [`Self::code`]); a client branching on
    /// the error code handles it once. Kept as its own variant (rather than
    /// merged into a single one) because its call sites (missing
    /// control-plane arguments like `host.tool_publish`'s `name`) are
    /// distinct from the generic `tools/call` args-schema pre-check below,
    /// which still builds an [`AppError::Structured`] directly so its extra
    /// `data` fields survive.
    #[error("invalid arguments: {0}")]
    InvalidArgs(String),
    /// `host.tool_publish` rejected a spec referencing `secret.<name>` for
    /// a secret this tenant hasn't set (AC3).
    #[error("spec references unknown secret '{0}'")]
    SecretMissing(String),
    /// A [`KindError::Structured`] passed through unchanged: a kind-owned
    /// error code (e.g. `host_not_allowed`, `upstream_status`,
    /// `template_error`, `rate_limited`) its own acceptance criteria name
    /// directly. `data` carries extra fields (e.g. `retry_after_s`) merged
    /// into the JSON-RPC error's `data` alongside `error_code`.
    #[error("{message}")]
    Structured {
        code: &'static str,
        message: String,
        data: Value,
    },
    /// Requirement 3 / AC2: two or more simultaneously-invalid fields from
    /// one `Kind::validate_all` call, reported together instead of one per
    /// publish attempt. Built only by [`AppError::from_kind_violations`],
    /// never constructed directly -- `message` is a short summary computed
    /// there ("N fields invalid: a, b"); `errors` holds each violation
    /// already converted to its own `AppError` so [`Self::code`],
    /// [`Self::field_and_expected`] and [`Self::into_error_data`] can reuse
    /// the same per-field logic every other variant already has, keyed off
    /// `errors[0]` for the top-level (back-compat) `error_code`/`field`/
    /// `expected`/`example`, and `errors` in full for the new `data.errors`
    /// array an agent should actually read.
    #[error("{message}")]
    MultiInvalid {
        message: String,
        errors: Vec<AppError>,
    },
    /// PRD-mcphost-call-limits-honest requirement 1: names the deadline
    /// that actually applied to this call -- the declared `timeout_s`
    /// (bounded by the kind's own max) when the spec named one, otherwise
    /// [`crate::state::CALL_TIMEOUT`]'s default -- never a hardcoded "30s"
    /// that may not be what was actually enforced.
    #[error("call timed out after the {0}s deadline")]
    CallTimeout(u64),
    #[error("storage error: {0}")]
    Storage(String),
    #[error("internal error: {0}")]
    Internal(String),
    #[error(
        "registry-publish is disabled; set --registry-url / $MCPHOST_REGISTRY_URL to enable it"
    )]
    RegistryDisabled,
    #[error(
        "tenant's domain namespace is not verified; ask the operator to run admin.tenant_verify_namespace"
    )]
    NamespaceUnverified,
    #[error("registry API rejected the publish: {0}")]
    RegistryRejected(String),
    /// PRD-mcphost-sharing requirement 4 (AC6): distinct from the generic
    /// `billing::quota_exceeded` (code `quota_exceeded`) because the PRD
    /// pins this one's wire code to exactly `share_quota_exceeded`, naming
    /// the limit.
    #[error("tenant already shares {used} tools, the plan maximum of {limit}")]
    ShareQuotaExceeded { limit: i64, used: i64 },
    /// PRD-mcphost-sharing requirement 1: `host.group.add`/`host.group.remove`
    /// on a group name this tenant hasn't `host.group.create`d.
    #[error("group not found: {0}")]
    GroupNotFound(String),
    /// PRD-mcphost-handoff-token requirement 2 / AC2: `host.redeem` on a
    /// token matching no row -- never distinguished from "existed once,
    /// long expired and swept" (there is no sweep), same "guessed value
    /// reads identically to a bad one" posture as [`AppError::TenantKeyInvalid`].
    #[error("handoff token was not recognized")]
    HandoffTokenInvalid,
    /// PRD-mcphost-handoff-token requirement 2 / AC2: a second redemption
    /// of an already-claimed token -- distinct code from
    /// [`AppError::HandoffTokenExpired`] so a client (or the AC4
    /// integration test replaying context A's transcript) can tell "this
    /// token worked once already" from "this token's window closed".
    #[error("handoff token has already been redeemed")]
    HandoffTokenRedeemed,
    /// PRD-mcphost-handoff-token requirement 2 / AC2: redemption attempted
    /// after `state::HANDOFF_TOKEN_TTL_SECS` elapsed since issuance.
    #[error("handoff token has expired")]
    HandoffTokenExpired,
}

impl AppError {
    /// The machine-readable code every AC-facing test matches on.
    pub fn code(&self) -> &'static str {
        match self {
            // PRD-mcphost-auth-error-names-argument requirement 3 / AC4:
            // renamed from "unauthorized" -- the header path's own code,
            // distinct from the two argument-path codes below now that
            // there are three ways to fail auth instead of one.
            AppError::Unauthorized => "bearer_invalid",
            AppError::TenantKeyMissing => "tenant_key_missing",
            AppError::TenantKeyInvalid => "tenant_key_invalid",
            AppError::Forbidden => "forbidden",
            AppError::TenantDisabled => "tenant_disabled",
            AppError::RateLimited => "rate_limited",
            AppError::ToolNotFound(_) => "tool_not_found",
            AppError::TenantNotFound(_) => "tenant_not_found",
            AppError::InvalidParams(_) => "invalid_params",
            AppError::UnknownKind { .. } => "unknown_kind",
            AppError::InvalidToolName(_) => "invalid_tool_name",
            AppError::SpecTooLarge(_) => "spec_too_large",
            AppError::TooManyTools(_) => "too_many_tools",
            AppError::InvalidSpec(_) => "invalid_spec",
            // PRD-mcphost-surface-fluidity requirement 2: one code for bad
            // arguments -- this used to be a distinct wire code, now it's
            // the same "args_invalid" every other bad-argument rejection in
            // this file already uses.
            AppError::InvalidArgs(_) => "args_invalid",
            AppError::SecretMissing(_) => "secret_missing",
            AppError::Structured { code, .. } => code,
            AppError::MultiInvalid { errors, .. } => {
                errors.first().map(AppError::code).unwrap_or("invalid_spec")
            }
            AppError::CallTimeout(_) => "call_timeout",
            AppError::Storage(_) => "storage",
            AppError::Internal(_) => "internal",
            AppError::RegistryDisabled => "registry_disabled",
            AppError::NamespaceUnverified => "namespace_unverified",
            AppError::RegistryRejected(_) => "registry_rejected",
            AppError::ShareQuotaExceeded { .. } => "share_quota_exceeded",
            AppError::GroupNotFound(_) => "group_not_found",
            AppError::HandoffTokenInvalid => "handoff_token_invalid",
            AppError::HandoffTokenRedeemed => "handoff_token_redeemed",
            AppError::HandoffTokenExpired => "handoff_token_expired",
        }
    }

    fn jsonrpc_code(&self) -> ErrorCode {
        match self {
            AppError::ToolNotFound(_) | AppError::TenantNotFound(_) | AppError::GroupNotFound(_) => {
                ErrorCode::RESOURCE_NOT_FOUND
            }
            AppError::UnknownKind { .. }
            | AppError::InvalidToolName(_)
            | AppError::SpecTooLarge(_)
            | AppError::TooManyTools(_)
            | AppError::InvalidSpec(_)
            | AppError::InvalidArgs(_)
            | AppError::InvalidParams(_)
            | AppError::ShareQuotaExceeded { .. }
            | AppError::SecretMissing(_) => ErrorCode::INVALID_PARAMS,
            AppError::Storage(_)
            | AppError::Internal(_)
            | AppError::CallTimeout(_)
            | AppError::RegistryRejected(_) => ErrorCode::INTERNAL_ERROR,
            AppError::Unauthorized
            | AppError::TenantKeyMissing
            | AppError::TenantKeyInvalid
            | AppError::Forbidden
            | AppError::TenantDisabled
            | AppError::RateLimited
            | AppError::RegistryDisabled
            | AppError::NamespaceUnverified
            | AppError::HandoffTokenInvalid
            | AppError::HandoffTokenRedeemed
            | AppError::HandoffTokenExpired => ErrorCode::INVALID_REQUEST,
            // `host_not_allowed`/`args_invalid`/`template_error` are caller
            // (or spec-author) input problems; `rate_limited` mirrors
            // AppError::RateLimited above; the remaining `upstream_*` /
            // `response_too_large` codes are upstream-side failures the
            // host itself didn't cause, so they line up with the other
            // INTERNAL_ERROR-mapped variants above.
            AppError::Structured { code, .. } => match *code {
                "rate_limited" => ErrorCode::INVALID_REQUEST,
                // PRD-mcphost-spec-output-paths requirement 1: a structured
                // `invalid_spec` (kinds::http/python's own `parse_spec` and
                // `normalize_outputs`) is exactly the same caller-input
                // problem `AppError::InvalidSpec` already maps to
                // INVALID_PARAMS above -- it must not fall into this match's
                // `_ => INTERNAL_ERROR` default just because it arrives via
                // `KindError::Structured` instead of `KindError::InvalidSpec`.
                "host_not_allowed" | "args_invalid" | "template_error" | "kind_mismatch"
                | "invalid_spec"
                // PRD-mcphost-python-kind-plain-env requirement 3 (AC4): a
                // publish/secret_set naming a colliding env/secret key is
                // the same caller-input problem as `invalid_spec` above.
                | "env_secret_collision" => ErrorCode::INVALID_PARAMS,
                // PRD-mcphost-tenant-state requirement 1/4: a schema
                // violation, a quota overrun, and an undeclared table are
                // all caller-input problems (a bad `host.state.insert`
                // payload, a write past a plan's quota, a typo'd table
                // name) -- the same INVALID_PARAMS bucket as `invalid_spec`
                // above, not an internal failure.
                "state_schema_violation" | "state_quota_exceeded" | "state_table_not_found" => {
                    ErrorCode::INVALID_PARAMS
                }
                // PRD-mcphost-schedules requirements 2/4: a bad cron
                // expression, an over-quota schedules_max, or a too-short
                // schedule_min_interval_s are all caller-input problems,
                // same INVALID_PARAMS bucket as the `state_*` group above;
                // `trigger_not_found` mirrors `tool_not_found`/
                // `tenant_not_found`'s own RESOURCE_NOT_FOUND.
                "trigger_invalid" | "trigger_quota_exceeded" | "trigger_interval_too_short" => {
                    ErrorCode::INVALID_PARAMS
                }
                "trigger_not_found" => ErrorCode::RESOURCE_NOT_FOUND,
                // PRD-mcphost-inbound-events: `host.trigger.test`'s own
                // caller-input problems -- a bad signature/payload, a rate
                // ceiling, an oversized body -- same INVALID_PARAMS/
                // INVALID_REQUEST split the `state_*`/`trigger_*` groups
                // above already use; `hook_not_found` mirrors
                // `trigger_not_found`'s RESOURCE_NOT_FOUND (only reachable
                // here via `host.trigger.test` on an unknown/wrong-kind
                // trigger -- `POST /hooks/...` itself never goes through
                // this jsonrpc mapping, it builds its own HTTP status).
                "signature_invalid" | "event_body_too_large" => ErrorCode::INVALID_PARAMS,
                "events_rate_limited" => ErrorCode::INVALID_REQUEST,
                "hook_not_found" => ErrorCode::RESOURCE_NOT_FOUND,
                _ => ErrorCode::INTERNAL_ERROR,
            },
            AppError::MultiInvalid { errors, .. } => errors
                .first()
                .map(AppError::jsonrpc_code)
                .unwrap_or(ErrorCode::INVALID_PARAMS),
        }
    }

    /// Splits a "<field>: <rest>" convention message -- the pattern every
    /// `KindError::InvalidSpec`/`InvalidArgs`/`Structured` message in this
    /// crate already follows (`method: must be one of ...`, `source: must
    /// not be empty`, `url: rendered host '...' is not publicly callable`)
    /// -- into the field name and the human-readable "expected" tail.
    /// Guards against splitting an ordinary sentence that happens to
    /// contain ": " (e.g. "near line 3: unexpected token") by requiring the
    /// candidate field to look like an identifier/JSON-pointer segment, not
    /// prose (no spaces).
    fn split_field(message: &str) -> Option<(&str, &str)> {
        let (field, rest) = message.split_once(": ")?;
        if field.is_empty() || field.contains(' ') {
            return None;
        }
        Some((field, rest.trim()))
    }

    /// The `field`/`expected` pair for every rejection this crate can
    /// return (requirement 2 / AC2, AC5): named `AppError` variants get a
    /// hand-written field and expectation naming the exact control-plane
    /// argument at fault; `InvalidSpec`/`InvalidArgs` and any `Structured`
    /// kind error fall back to the shared "<field>: <rest>"
    /// message convention via [`Self::split_field`] -- so a kind gains this
    /// for free by writing its error messages the way every kind already
    /// does, no enum change or call-site rewrite required.
    fn field_and_expected(&self) -> (Option<String>, Option<String>) {
        match self {
            AppError::InvalidToolName(name) => (
                Some("name".to_string()),
                Some(format!("must match ^[a-z][a-z0-9_]{{1,40}}$; got '{name}'")),
            ),
            AppError::UnknownKind { registered, .. } => (
                Some("kind".to_string()),
                Some(format!("must be one of {registered:?}")),
            ),
            AppError::SpecTooLarge(n) => (
                Some("spec".to_string()),
                Some(format!(
                    "serializes to {n} bytes; must be at most {} bytes",
                    crate::state::MAX_SPEC_BYTES
                )),
            ),
            AppError::SecretMissing(name) => (
                Some("spec".to_string()),
                Some(format!(
                    "references secret.{name}, which this tenant has not set; \
                     call host.secret_set first"
                )),
            ),
            AppError::InvalidArgs(m) | AppError::InvalidSpec(m) => {
                match Self::split_field(m) {
                    Some((field, expected)) => {
                        (Some(field.to_string()), Some(expected.to_string()))
                    }
                    None => (None, Some(m.clone())),
                }
            }
            AppError::Structured { message, data, .. } => {
                let field_from_data = data
                    .get("field")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                // PRD-mcphost-spec-output-paths requirement 1: a structured
                // `invalid_spec` already carries its own clean `expected`
                // phrase in `data` (distinct from `got`/`example`) -- prefer
                // it over the generic "<field>: <rest>" message-split below,
                // which would otherwise clobber it with the *whole* message
                // (this variant's own `message` deliberately embeds an
                // `"invalid spec: "` prefix per requirement 1's wire format,
                // which defeats `split_field`'s "no spaces in the field"
                // guard and falls through to the whole-message fallback).
                // No existing `Structured` error sets `data.expected`, so
                // this is purely additive for every caller that predates
                // this PRD.
                let expected_from_data = data
                    .get("expected")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let split = Self::split_field(message);
                let field = field_from_data.or_else(|| split.map(|(f, _)| f.to_string()));
                let expected = expected_from_data
                    .or_else(|| split.map(|(_, e)| e.to_string()))
                    .or_else(|| Some(message.clone()));
                (field, expected)
            }
            // Back-compat top-level field/expected (requirement 2): the
            // first violation's own, same as if it had been the only one.
            // Every violation (including this first one again) also gets
            // its own entry in `into_error_data`'s `errors` array below.
            AppError::MultiInvalid { errors, .. } => errors
                .first()
                .map(AppError::field_and_expected)
                .unwrap_or((None, None)),
            _ => (None, None),
        }
    }

    pub fn into_error_data(self) -> ErrorData {
        let code = self.code();
        let jsonrpc_code = self.jsonrpc_code();
        let message = self.to_string();
        let (field, expected) = self.field_and_expected();
        let example = field.as_deref().and_then(field_example);
        let mut obj: Map<String, Value> = match &self {
            AppError::Structured { data, .. } if data.is_object() => {
                data.as_object().cloned().unwrap_or_default()
            }
            _ => Map::new(),
        };
        obj.insert("error_code".to_string(), json!(code));
        if let Some(field) = field {
            obj.insert("field".to_string(), json!(field));
        }
        if let Some(expected) = expected {
            obj.insert("expected".to_string(), json!(expected));
        }
        if let Some(example) = example {
            obj.insert("example".to_string(), example);
        }
        // Requirement 3 / AC2: every simultaneously-failing field, not just
        // the first -- one entry per violation, each with its own
        // `field`/`expected`/`example` built the same way the top-level
        // ones above are.
        if let AppError::MultiInvalid { errors, .. } = &self {
            let entries: Vec<Value> = errors
                .iter()
                .map(|e| {
                    let (field, expected) = e.field_and_expected();
                    let example = field.as_deref().and_then(field_example);
                    let mut entry = Map::new();
                    if let Some(field) = field {
                        entry.insert("field".to_string(), json!(field));
                    }
                    if let Some(expected) = expected {
                        entry.insert("expected".to_string(), json!(expected));
                    }
                    if let Some(example) = example {
                        entry.insert("example".to_string(), example);
                    }
                    Value::Object(entry)
                })
                .collect();
            obj.insert("errors".to_string(), Value::Array(entries));
        }
        // Requirement 2 / requirement 4: every rejection names the
        // quickstart tool that gives the caller a filled-in working
        // example for whatever it was trying to publish.
        obj.insert("docs".to_string(), json!("host.quickstart"));
        ErrorData::new(jsonrpc_code, message, Some(Value::Object(obj)))
    }

    /// Requirement 3 / AC2: builds the single-field error exactly as before
    /// when [`Kind::validate_all`](crate::kinds::Kind::validate_all) found
    /// exactly one violation (unchanged wire shape -- every existing test
    /// asserting on it keeps passing), or an [`AppError::MultiInvalid`]
    /// carrying every violation at once when it found more than one, so
    /// `host.tool_publish` reports every failing field in a single
    /// rejection instead of one per attempt. `None` when `violations` is
    /// empty (the spec is valid).
    /// PRD-mcphost-sandbox-ready requirement 3 (AC3/AC7): a python-kind
    /// `host.tool_publish`/`host.tool_test`/`host.tool_run` on a host whose
    /// sandbox self-test is failing, rejected before the spec is ever
    /// evaluated -- never routed through `AppError::Internal` the way
    /// `KindError::Exec` normally maps (see `From<KindError>` below), which
    /// is exactly the bug this PRD's TL;DR describes: a real sandbox
    /// failure surfacing as an opaque internal error an agent retries
    /// uselessly. `data.docs` is filled in by `into_error_data`'s existing
    /// unconditional `"host.quickstart"` insert, same as every other
    /// rejection this crate returns -- not overridden here.
    pub fn sandbox_unavailable(status: &crate::sandbox::SandboxStatus) -> Self {
        AppError::Structured {
            code: "sandbox_unavailable",
            message: "the python kind is unavailable on this host; the spec was not evaluated"
                .to_string(),
            data: json!({
                "mechanism": status.mechanism.as_str(),
                "detail": status.detail,
                "alternatives": ["echo", "http"],
            }),
        }
    }

    /// PRD-mcphost-python-kind-plain-env requirement 3 (AC4): a publish's
    /// own `env` name collides with a secret already known for this tenant
    /// -- secrets are tenant-scoped (not per-tool), so this is checked
    /// against every secret the tenant has ever set, not just ones this
    /// spec itself references.
    pub fn env_collides_with_secret(key: &str) -> Self {
        AppError::Structured {
            code: "env_secret_collision",
            message: format!(
                "env.{key}: '{key}' collides with an existing secret of the same name for this tenant"
            ),
            data: json!({"field": format!("env.{key}"), "key": key, "rule": "secret_collision"}),
        }
    }

    /// PRD-mcphost-python-kind-plain-env requirement 3 (AC4), the symmetric
    /// direction: `host.secret_set` refuses a new secret name that collides
    /// with any of this tenant's already-published tools' `env` entries.
    pub fn secret_collides_with_env(key: &str) -> Self {
        AppError::Structured {
            code: "env_secret_collision",
            message: format!(
                "'{key}' collides with a published tool's env entry of the same name"
            ),
            data: json!({"field": "name", "key": key, "rule": "env_collision"}),
        }
    }

    /// PRD-mcphost-tool-kind-honor requirement 1 (AC2): the caller's own
    /// requested `kind` disagrees with what the spec's shape can only be
    /// (see [`crate::kinds::infer::infer_kind_signal`]) -- refused before
    /// the spec is ever validated against (let alone stored under) the
    /// wrong `Kind` impl, naming the disagreeing element so the caller can
    /// fix either the spec or the request, instead of getting whichever
    /// kind's own `validate` happened to fail with a generic message (or,
    /// worse, silently succeed under the wrong kind).
    pub fn kind_mismatch(requested: &str, signal: &KindSignal) -> Self {
        AppError::Structured {
            code: "kind_mismatch",
            message: format!(
                "kind: requested '{requested}' but the spec's shape is only valid as '{}': {}",
                signal.kind, signal.reason
            ),
            data: json!({
                "requested": requested,
                "inferred": signal.kind,
                "reason": signal.reason,
            }),
        }
    }

    pub fn from_kind_violations(violations: Vec<KindError>) -> Option<AppError> {
        let mut iter = violations.into_iter();
        let first = AppError::from(iter.next()?);
        let rest: Vec<AppError> = iter.map(AppError::from).collect();
        if rest.is_empty() {
            return Some(first);
        }
        let mut errors = vec![first];
        errors.extend(rest);
        let fields: Vec<String> = errors
            .iter()
            .filter_map(|e| e.field_and_expected().0)
            .collect();
        let message = format!("{} fields invalid: {}", errors.len(), fields.join(", "));
        Some(AppError::MultiInvalid { message, errors })
    }
}

impl From<KindError> for AppError {
    fn from(e: KindError) -> Self {
        match e {
            KindError::InvalidSpec(m) => AppError::InvalidSpec(m),
            KindError::InvalidArgs(m) => AppError::InvalidArgs(m),
            KindError::Structured {
                code,
                message,
                data,
            } => AppError::Structured {
                code,
                message,
                data,
            },
            KindError::Exec(m) => AppError::Internal(m),
        }
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        AppError::Storage(e.to_string())
    }
}
