//! `mcphost`'s own error taxonomy, and its mapping onto MCP JSON-RPC errors.
//!
//! Every acceptance criterion that names an error code (`tenant_disabled`,
//! `tool_not_found`, `rate_limited`, ...) gets it from [`AppError::code`],
//! carried in the JSON-RPC error's `data.error_code` field so a caller (or a
//! test) can match on it without parsing prose.

use rmcp::model::{ErrorCode, ErrorData};
use serde_json::json;

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
    #[error("call timed out after the 30s deadline")]
    CallTimeout,
    #[error("storage error: {0}")]
    Storage(String),
    #[error("internal error: {0}")]
    Internal(String),
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
            AppError::UnknownKind { .. } => "unknown_kind",
            AppError::InvalidToolName(_) => "invalid_tool_name",
            AppError::SpecTooLarge(_) => "spec_too_large",
            AppError::TooManyTools(_) => "too_many_tools",
            AppError::InvalidSpec(_) => "invalid_spec",
            AppError::InvalidArgs(_) => "invalid_args",
            AppError::CallTimeout => "call_timeout",
            AppError::Storage(_) => "storage",
            AppError::Internal(_) => "internal",
        }
    }

    fn jsonrpc_code(&self) -> ErrorCode {
        match self {
            AppError::ToolNotFound(_) => ErrorCode::RESOURCE_NOT_FOUND,
            AppError::UnknownKind { .. }
            | AppError::InvalidToolName(_)
            | AppError::SpecTooLarge(_)
            | AppError::TooManyTools(_)
            | AppError::InvalidSpec(_)
            | AppError::InvalidArgs(_) => ErrorCode::INVALID_PARAMS,
            AppError::Storage(_) | AppError::Internal(_) | AppError::CallTimeout => {
                ErrorCode::INTERNAL_ERROR
            }
            AppError::Unauthorized
            | AppError::Forbidden
            | AppError::TenantDisabled
            | AppError::RateLimited => ErrorCode::INVALID_REQUEST,
        }
    }

    pub fn into_error_data(self) -> ErrorData {
        let code = self.code();
        let message = self.to_string();
        ErrorData::new(
            self.jsonrpc_code(),
            message,
            Some(json!({"error_code": code})),
        )
    }
}

impl From<KindError> for AppError {
    fn from(e: KindError) -> Self {
        match e {
            KindError::InvalidSpec(m) => AppError::InvalidSpec(m),
            KindError::InvalidArgs(m) => AppError::InvalidArgs(m),
            KindError::Exec(m) => AppError::Internal(m),
        }
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        AppError::Storage(e.to_string())
    }
}
