//! AC5: `mcphost examples schema` is byte-identical to
//! `examples/example.schema.json`, both enums list exactly
//! `EXAMPLE_USE_CASES` / `CLIENT_TAGS`, and a tag added to a Rust slice
//! without regenerating is caught.

use std::path::Path;
use std::process::Command;

use mcphost::examples::{CLIENT_TAGS, EXAMPLE_USE_CASES, SCHEMA_FILE, render_schema, render_schema_with};

fn committed() -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("examples").join(SCHEMA_FILE))
        .expect("examples/example.schema.json is committed")
}

fn enum_of(schema: &serde_json::Value, field: &str) -> Vec<String> {
    schema["properties"]["example"]["properties"][field]["items"]["enum"]
        .as_array()
        .unwrap_or_else(|| panic!("no enum for {field}"))
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn schema_subcommand_output_equals_the_committed_file() {
    let out = Command::new(env!("CARGO_BIN_EXE_mcphost")).args(["examples", "schema"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8(out.stdout).unwrap(), committed(), "run `mcphost examples schema > examples/example.schema.json`");
}

#[test]
fn both_enums_list_exactly_the_rust_slices() {
    let schema: serde_json::Value = serde_json::from_str(&committed()).unwrap();
    assert_eq!(enum_of(&schema, "use_case"), EXAMPLE_USE_CASES);
    assert_eq!(enum_of(&schema, "clients"), CLIENT_TAGS);
}

#[test]
fn a_tag_added_to_a_slice_without_regenerating_fails_the_drift_check() {
    assert_eq!(render_schema(), committed(), "baseline must be in sync");
    let mut use_cases = EXAMPLE_USE_CASES.to_vec();
    use_cases.push("new-use-case");
    assert_ne!(render_schema_with(&use_cases, CLIENT_TAGS), committed());
    let mut clients = CLIENT_TAGS.to_vec();
    clients.push("new-client");
    assert_ne!(render_schema_with(EXAMPLE_USE_CASES, &clients), committed());
}

#[test]
fn every_committed_manifest_validates_against_the_schema() {
    let schema: serde_json::Value = serde_json::from_str(&committed()).unwrap();
    let validator = jsonschema::validator_for(&schema).expect("schema compiles");
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    for dir in mcphost::examples::example_dirs(&root).unwrap() {
        let text = std::fs::read_to_string(dir.join(mcphost::examples::MANIFEST_FILE)).unwrap();
        let doc: serde_json::Value = toml::from_str(&text).unwrap();
        assert!(validator.is_valid(&doc), "{} does not validate against the schema", dir.display());
    }
}
