//! `mcphost`'s own error taxonomy, and its mapping onto MCP JSON-RPC errors.
//!
//! Every acceptance criterion that names an error code (`tenant_disabled`,
//! `tool_not_found`, `rate_limited`, ...) gets it from [`AppError::code`],
//! carried in the JSON-RPC error's `data.error_code` field so a caller (or a
//! test) can match on it without parsing prose.

use rmcp::model::{ErrorCode, ErrorData};
use serde_json::{Value, json};

use crate::kinds::KindError;

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

    pub fn into_error_data(self) -> ErrorData {
        let code = self.code();
        let jsonrpc_code = self.jsonrpc_code();
        let message = self.to_string();
        let data = match self {
            AppError::Structured { data, .. } if data.is_object() => {
                let mut obj = data.as_object().cloned().unwrap_or_default();
                obj.insert("error_code".to_string(), json!(code));
                Value::Object(obj)
            }
            _ => json!({"error_code": code}),
        };
        ErrorData::new(jsonrpc_code, message, Some(data))
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
