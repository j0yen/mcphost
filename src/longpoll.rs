//! PRD-mcphost-msg-wait-long-poll-contract
//! The one place the long-poll ceiling lives: `host.msg.wait`,
//! `host.runs.wait` and `host.ask.wait` clamp through it, and quickstart,
//! the tool descriptors and llms.txt render it.

use serde_json::{Value, json};
use std::time::Instant;

/// The longest one wait call blocks, in seconds.
pub const WAIT_MAX_S: i64 = 25;

/// A wait call's default `timeout_s`.
pub const WAIT_DEFAULT_S: i64 = 20;

/// Clamp a caller's `timeout_s` (or the default) to `1..=WAIT_MAX_S`.
pub fn clamp_timeout_s(requested: Option<i64>) -> i64 {
    requested.unwrap_or(WAIT_DEFAULT_S).clamp(1, WAIT_MAX_S)
}

/// Read `timeout_s` from a wait call's args: absent or null is the default,
/// an integer is clamped, anything else is `args_invalid`.
pub fn timeout_s_arg(args: &Value) -> Result<i64, crate::errors::AppError> {
    match args.get("timeout_s") {
        None | Some(Value::Null) => Ok(clamp_timeout_s(None)),
        Some(v) => v.as_i64().map(|n| clamp_timeout_s(Some(n))).ok_or_else(|| {
            let args_err = crate::kinds::ArgsError {
                phase: "args_coercion",
                instance_path: "/timeout_s".to_string(),
                argument: Some("timeout_s".to_string()),
                field: Some("timeout_s".to_string()),
                expected_type: Some("integer".to_string()),
                actual_type: None,
                got: v.to_string(),
                docs: "host.msg.wait",
                raw: v.to_string(),
            };
            let mut data = args_err.data();
            data["max"] = json!(WAIT_MAX_S);
            crate::errors::AppError::Structured {
                code: "args_invalid",
                message: args_err.message(),
                data,
            }
        }),
    }
}

/// The field set every wait response carries: `timed_out`, `waited_ms`,
/// `timeout_s_applied`, merged into `body`.
pub fn wait_response(mut body: Value, timed_out: bool, started: Instant, timeout_s_applied: i64) -> Value {
    if let Some(obj) = body.as_object_mut() {
        obj.insert("timed_out".to_string(), json!(timed_out));
        obj.insert("waited_ms".to_string(), json!(started.elapsed().as_millis() as u64));
        obj.insert("timeout_s_applied".to_string(), json!(timeout_s_applied));
    }
    body
}
