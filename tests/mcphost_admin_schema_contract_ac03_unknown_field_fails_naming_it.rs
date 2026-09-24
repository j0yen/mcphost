//! PRD-mcphost-admin-schema-contract
//! AC3 (P0) — Given a row with an unexpected additional field of unknown
//! type, When validated with `additionalProperties: false` in the schema,
//! Then the test fails naming the field.
//!
//! This test itself must not fail on such a row -- it asserts the
//! validator REJECTS it and that the rejection names the offending field,
//! proving `additionalProperties: false` is actually wired into both v1
//! schemas (a schema that dropped that keyword would let this row through
//! silently).

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

fn crate_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn load_schema(rel: &str) -> Value {
    let path: PathBuf = crate_root().join(rel);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read schema {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse schema {}: {e}", path.display()))
}

fn valid_tenant_row() -> Value {
    json!({
        "tenant": "t_abc123",
        "display_name": "Valid Tenant",
        "created_at": "2026-09-24T00:00:00Z",
        "disabled": false,
        "disabled_reason": null,
        "synthetic": null,
        "source_class": "external",
        "client_name": null,
        "client_version": null,
        "owner_verified": false,
    })
}

#[test]
fn tenants_row_with_unexpected_field_fails_naming_it() {
    let schema = load_schema("schemas/admin/tenants.v1.json");
    let validator = jsonschema::validator_for(&schema).expect("compile tenants.v1.json");

    let good = json!({"schema_version": 1, "tenants": [valid_tenant_row()]});
    assert!(
        validator.is_valid(&good),
        "sanity: a well-formed listing must validate: {good:?}"
    );

    let mut bad_row = valid_tenant_row();
    bad_row
        .as_object_mut()
        .expect("row object")
        .insert("mystery_field".to_string(), json!(42));
    let bad = json!({"schema_version": 1, "tenants": [bad_row]});

    let errors: Vec<String> = validator.iter_errors(&bad).map(|e| e.to_string()).collect();
    assert!(
        !errors.is_empty(),
        "a row with an unexpected additional field must fail v1 schema validation"
    );
    assert!(
        errors.iter().any(|e| e.contains("mystery_field")),
        "the failure must name the unexpected field 'mystery_field': {errors:?}"
    );
}

#[test]
fn usage_row_with_unexpected_field_fails_naming_it() {
    let schema = load_schema("schemas/admin/usage.v1.json");
    let validator = jsonschema::validator_for(&schema).expect("compile usage.v1.json");

    let good_row = json!({
        "tenant": "t_abc123",
        "tool": "hello",
        "calls": 1,
        "errors": 0,
        "p50_ms": 1.5,
        "p95_ms": 2.5,
    });
    let good = json!({
        "schema_version": 1,
        "window": "24h",
        "usage": [good_row.clone()],
        "db_bytes": 0,
        "db_page_free_bytes": 0,
        "rows_by_table": {},
        "last_prune": null,
        "exports_today": 0,
        "tools_by_network": {},
        "tools_by_advisory_state": {"clean": 0, "advisory": 0},
    });
    assert!(
        validator.is_valid(&good),
        "sanity: a well-formed usage listing must validate: {good:?}"
    );

    let mut bad_row = good_row;
    bad_row
        .as_object_mut()
        .expect("row object")
        .insert("unexpected_debug_field".to_string(), json!({"nested": true}));
    let mut bad = good.clone();
    bad["usage"] = json!([bad_row]);

    let errors: Vec<String> = validator.iter_errors(&bad).map(|e| e.to_string()).collect();
    assert!(
        !errors.is_empty(),
        "a usage row with an unexpected additional field must fail v1 schema validation"
    );
    assert!(
        errors.iter().any(|e| e.contains("unexpected_debug_field")),
        "the failure must name the unexpected field 'unexpected_debug_field': {errors:?}"
    );
}
