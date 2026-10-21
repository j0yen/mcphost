//! PRD-mcphost-invalid-spec-error-carries-kind-fields
//! AC5 (P0) — Given the error-codes registry and contract, When
//! `invalid_spec` `data` gains these keys, Then the generated contract,
//! docs/errors.md and both SDKs carry them with no hand edit and the registry
//! drift test passes.

use mcphost::error_registry::{ERROR_CODES, catalog_json, lookup};
use mcphost::kinds::kinddoc::invalid_spec_data;

fn read(rel: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

#[test]
fn spkind_ac05_registry_carries_data_keys() {
    let row = lookup("invalid_spec").expect("invalid_spec is a registry row");
    assert!(!row.data_keys.is_empty(), "invalid_spec declares its data keys in the registry");

    // Every key the refusal builder emits is declared on the registry row.
    let registry = mcphost::kinds::production_registry();
    for name in registry.names() {
        let kind = registry.get(name).expect("registered kind resolves");
        let data = invalid_spec_data(kind.as_ref(), None, "probe");
        for key in data.as_object().expect("object").keys() {
            assert!(row.data_keys.contains(&key.as_str()), "kind {name}: data.{key} is not in the registry row");
        }
    }

    // The contract: /errors.json and host.errors rows carry them.
    let catalog = catalog_json("http://localhost");
    let json_row = catalog["codes"].as_array().unwrap().iter().find(|r| r["code"] == "invalid_spec").unwrap();
    assert_eq!(json_row["data_keys"], serde_json::json!(row.data_keys));

    // The checked-in generated surfaces name every key of every declaring code.
    let errors_md = read("docs/errors.md");
    let py = read("sdk/python/mcphost_sdk/errors.py");
    let ts = read("sdk/ts/src/errors.ts");
    for entry in ERROR_CODES.iter().filter(|e| !e.data_keys.is_empty()) {
        for key in entry.data_keys {
            assert!(errors_md.contains(&format!("`{key}`")), "docs/errors.md lacks {key} of {}", entry.code);
            assert!(py.contains(&format!("\"{key}\"")), "python SDK lacks {key} of {}", entry.code);
            assert!(ts.contains(&format!("\"{key}\"")), "ts SDK lacks {key} of {}", entry.code);
        }
    }
    // And they are the generator's output, so a hand edit or a stale file fails.
    assert_eq!(errors_md, mcphost::retry_policy::render_doc(), "docs/errors.md is stale");
    let contract = read("contracts/host-tools.v1.json");
    let py_files = mcphost::sdk_python::render(contract.as_bytes()).expect("render python sdk");
    assert_eq!(py_files["errors.py"], py, "sdk/python errors.py is stale");
    let ts_files = mcphost::r#gen::sdk_ts::render(contract.as_bytes()).expect("render ts sdk");
    assert_eq!(ts_files["errors.ts"], ts, "sdk/ts errors.ts is stale");
}

/// Twin of the key check above for the class itself: every registry code has
/// a generated class in BOTH SDKs (they share `sdk_python::all_error_codes`),
/// so a code declared with `err_code!` outside `AppError` cannot be missing.
#[test]
fn spkind_ac05_every_registry_code_has_a_class_in_both_sdks() {
    let py = read("sdk/python/mcphost_sdk/errors.py");
    let ts = read("sdk/ts/src/errors.ts");
    for entry in ERROR_CODES.iter() {
        let class = mcphost::sdk_python::camel(entry.code);
        assert!(py.contains(&format!("class {class}(MCPHostError)")), "python SDK has no class for {}", entry.code);
        assert!(ts.contains(&format!("export class {class} extends MCPHostError")), "ts SDK has no class for {}", entry.code);
    }
}
