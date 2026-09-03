//! The `Kind` conformance suite (PRD requirement 5, AC17): a reusable check
//! any `Kind` implementation — this crate's `echo`, and the REST-wrapper and
//! code kinds shipped by the feature PRDs that extend this crate's
//! registry — must pass. Exposed from the library so those crates can run
//! it against their own kinds in their own `tests/kind_conformance.rs`,
//! per the PRD's technical considerations.

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
