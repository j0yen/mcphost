//! The `Kind` conformance suite (PRD requirement 5, AC17): a reusable check
//! any `Kind` implementation — this crate's `echo`, and the REST-wrapper and
//! code kinds shipped by the feature PRDs that extend this crate's
//! registry — must pass. Exposed from the library so those crates can run
//! it against their own kinds in their own copy of this crate's
//! `tests/ac17_kind_conformance.rs`, per the PRD's technical considerations.

use serde_json::Value;

use super::{CallCtx, Kind};

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
    let error_data = crate::errors::AppError::sandbox_unavailable(status).into_error_data();
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
    let alternatives_ok = data
        .get("alternatives")
        .and_then(Value::as_array)
        .is_some_and(|a| a.iter().any(|v| v == "echo") && a.iter().any(|v| v == "http"));
    if !alternatives_ok {
        return Err(ConformanceFailure {
            method: "sandbox_unavailable",
            reason: format!("data.alternatives must include 'echo' and 'http', got: {data}"),
        });
    }
    Ok(())
}
