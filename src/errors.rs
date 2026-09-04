//! `mcphost`'s own error taxonomy, and its mapping onto MCP JSON-RPC errors.
//!
//! Every acceptance criterion that names an error code (`tenant_disabled`,
//! `tool_not_found`, `rate_limited`, ...) gets it from [`AppError::code`],
//! carried in the JSON-RPC error's `data.error_code` field so a caller (or a
//! test) can match on it without parsing prose.

use rmcp::model::{ErrorCode, ErrorData};
use serde_json::{Map, Value, json};

use crate::kinds::KindError;

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
    #[error("invalid arguments: {0}")]
    InvalidArgs(String),
    /// The generic `tools/call` args-schema pre-check (`handler.rs`, run
    /// before `Kind::call`) failing a published tool's own `args_schema`.
    /// A distinct variant from [`AppError::InvalidArgs`] (used for missing
    /// *control-plane* arguments like `host.tool_publish`'s `name`) because
    /// the `http` kind's AC4 pins the wire code to exactly `args_invalid`.
    #[error("invalid arguments: {0}")]
    ArgsInvalid(String),
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
    #[error("call timed out after the 30s deadline")]
    CallTimeout,
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
}

impl AppError {
    /// The machine-readable code every AC-facing test matches on.
    pub fn code(&self) -> &'static str {
        match self {
            AppError::Unauthorized => "unauthorized",
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
            AppError::InvalidArgs(_) => "invalid_args",
            AppError::ArgsInvalid(_) => "args_invalid",
            AppError::SecretMissing(_) => "secret_missing",
            AppError::Structured { code, .. } => code,
            AppError::CallTimeout => "call_timeout",
            AppError::Storage(_) => "storage",
            AppError::Internal(_) => "internal",
            AppError::RegistryDisabled => "registry_disabled",
            AppError::NamespaceUnverified => "namespace_unverified",
            AppError::RegistryRejected(_) => "registry_rejected",
        }
    }

    fn jsonrpc_code(&self) -> ErrorCode {
        match self {
            AppError::ToolNotFound(_) | AppError::TenantNotFound(_) => {
                ErrorCode::RESOURCE_NOT_FOUND
            }
            AppError::UnknownKind { .. }
            | AppError::InvalidToolName(_)
            | AppError::SpecTooLarge(_)
            | AppError::TooManyTools(_)
            | AppError::InvalidSpec(_)
            | AppError::InvalidArgs(_)
            | AppError::ArgsInvalid(_)
            | AppError::InvalidParams(_)
            | AppError::SecretMissing(_) => ErrorCode::INVALID_PARAMS,
            AppError::Storage(_)
            | AppError::Internal(_)
            | AppError::CallTimeout
            | AppError::RegistryRejected(_) => ErrorCode::INTERNAL_ERROR,
            AppError::Unauthorized
            | AppError::Forbidden
            | AppError::TenantDisabled
            | AppError::RateLimited
            | AppError::RegistryDisabled
            | AppError::NamespaceUnverified => ErrorCode::INVALID_REQUEST,
            // `host_not_allowed`/`args_invalid`/`template_error` are caller
            // (or spec-author) input problems; `rate_limited` mirrors
            // AppError::RateLimited above; the remaining `upstream_*` /
            // `response_too_large` codes are upstream-side failures the
            // host itself didn't cause, so they line up with the other
            // INTERNAL_ERROR-mapped variants above.
            AppError::Structured { code, .. } => match *code {
                "rate_limited" => ErrorCode::INVALID_REQUEST,
                "host_not_allowed" | "args_invalid" | "template_error" => ErrorCode::INVALID_PARAMS,
                _ => ErrorCode::INTERNAL_ERROR,
            },
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
    /// argument at fault; `InvalidSpec`/`InvalidArgs`/`ArgsInvalid` and any
    /// `Structured` kind error fall back to the shared "<field>: <rest>"
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
            AppError::InvalidArgs(m) | AppError::ArgsInvalid(m) | AppError::InvalidSpec(m) => {
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
                let split = Self::split_field(message);
                let field = field_from_data.or_else(|| split.map(|(f, _)| f.to_string()));
                let expected = split
                    .map(|(_, e)| e.to_string())
                    .or_else(|| Some(message.clone()));
                (field, expected)
            }
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
        // Requirement 2 / requirement 4: every rejection names the
        // quickstart tool that gives the caller a filled-in working
        // example for whatever it was trying to publish.
        obj.insert("docs".to_string(), json!("host.quickstart"));
        ErrorData::new(jsonrpc_code, message, Some(Value::Object(obj)))
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
