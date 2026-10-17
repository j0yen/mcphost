//! PRD-mcphost-error-codes-generated-registry
//! AC2 (P0) — Given the pre-PRD `catalog_json`, `host.errors` and
//! `/help/<code>` outputs snapshotted with `version` stripped
//! (`tests/fixtures/errreg_snapshot.json`, taken from the registry before any
//! row moved), When the migrated registry renders the same calls for every
//! code, Then each body is byte-identical and `ERROR_CODES` iterates sorted by
//! code as before.
//!
//! Codes the PRD adds after the snapshot (the five 10-17 codes) are not in
//! the snapshot; they are filtered out of the catalog before comparing.

use mcphost::error_registry::{ERROR_CODES, call, catalog_json};
use mcphost::help::render_registry_help_page;
use serde_json::{Value, json};

fn snapshot() -> Value {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(root.join("tests/fixtures/errreg_snapshot.json")).expect("read snapshot");
    serde_json::from_str(&text).expect("snapshot is JSON")
}

#[test]
fn errreg_ac02_catalog_host_errors_and_help_bodies_are_byte_identical() {
    let snap = snapshot();
    let base = snap["base"].as_str().unwrap();
    let support = snap["support_url"].as_str().unwrap();
    let snap_codes: Vec<&str> = snap["catalog"]["codes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["code"].as_str().unwrap())
        .collect();
    assert!(snap_codes.len() > 150, "snapshot has only {} codes", snap_codes.len());

    // catalog_json minus `version` and minus codes added after the snapshot.
    let mut cat = catalog_json(base);
    cat.as_object_mut().unwrap().remove("version");
    cat["codes"].as_array_mut().unwrap().retain(|r| snap_codes.contains(&r["code"].as_str().unwrap()));
    assert_eq!(
        serde_json::to_string(&cat).unwrap(),
        serde_json::to_string(&snap["catalog"]).unwrap(),
        "catalog_json drifted from the pre-PRD snapshot"
    );

    for code in &snap_codes {
        let body = call(base, &json!({ "code": code })).expect("host.errors {code}");
        assert_eq!(
            serde_json::to_string(&body).unwrap(),
            serde_json::to_string(&snap["host_errors"][code]).unwrap(),
            "host.errors {code} drifted"
        );
        let entry = ERROR_CODES.iter().find(|e| e.code == *code).expect("code still registered");
        assert_eq!(
            render_registry_help_page(entry, Some(support)),
            snap["help"][code].as_str().unwrap(),
            "/help/{code} drifted"
        );
    }
}

#[test]
fn errreg_ac02_error_codes_iterate_sorted_by_code() {
    let codes: Vec<&str> = ERROR_CODES.iter().map(|e| e.code).collect();
    let mut sorted = codes.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(codes, sorted, "ERROR_CODES must be sorted by code with no duplicates");
    assert_eq!(ERROR_CODES.len(), codes.len());
}
