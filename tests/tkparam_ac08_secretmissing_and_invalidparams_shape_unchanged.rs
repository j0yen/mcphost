//! PRD-mcphost-tenant-key-missing-is-invalid-params
//! AC8 (P1) — Given a `SecretMissing` or `InvalidParams` rejection, When
//! `data` is inspected, Then its key set is unchanged from v0.60.34 (the
//! payload shape added for `TenantKeyMissing` is additive to that variant
//! only).
//!
//! Tested directly against `AppError::into_error_data` (not over HTTP)
//! because `host.tool_publish`'s own `SecretMissing` raise site wraps it
//! through `attach_gates` first, which already adds its own `gates` key
//! unrelated to this PRD -- the claim under test is narrower: this PRD's
//! `field_and_expected`/`field_example` additions target `TenantKeyMissing`
//! only and touch no other variant's own conversion.

use mcphost::errors::AppError;
use std::collections::BTreeSet;

fn keys(data: &serde_json::Value) -> BTreeSet<String> {
    data.as_object()
        .expect("data must be an object")
        .keys()
        .cloned()
        .collect()
}

#[test]
fn secret_missing_data_keys_are_unchanged() {
    let err = AppError::SecretMissing("missing".to_string()).into_error_data();
    let data = err.data.expect("SecretMissing must carry data");

    // PRD-mcphost-first-hour-support-surface requirement 1 (AC1) adds
    // `request_id` to every payload unconditionally -- additive to every
    // variant alike, so this AC8 pairing keeps meaning what it always has
    // ("no field/expected/example drift") with that one key folded in.
    assert_eq!(
        keys(&data),
        BTreeSet::from(["error_code", "field", "expected", "docs", "request_id"].map(String::from)),
        "secret_missing data keys must be exactly error_code/field/expected/docs/request_id, no example: {data:?}"
    );
}

#[test]
fn invalid_params_data_keys_are_unchanged() {
    let err =
        AppError::InvalidParams("scopes: must be an array of strings".to_string()).into_error_data();
    let data = err.data.expect("InvalidParams must carry data");

    // PRD-mcphost-first-hour-support-surface requirement 1 (AC1): see the
    // comment on the sibling assertion above -- same additive `request_id`.
    assert_eq!(
        keys(&data),
        BTreeSet::from(["error_code", "docs", "request_id"].map(String::from)),
        "invalid_params data keys must be exactly error_code/docs/request_id, no field/expected/example: {data:?}"
    );
}
