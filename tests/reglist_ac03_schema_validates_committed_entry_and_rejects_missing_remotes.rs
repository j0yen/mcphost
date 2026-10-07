//! PRD-mcphost-registry-listing
//! AC3 (P0) -- Given the pinned schema, When the test suite validates the
//! committed `server.json`, Then it passes, and an entry missing
//! `remotes` fails.
//!
//! The upstream schema's own `ServerDetail` leaves `remotes` optional (a
//! server can ship only `packages`), so validating a remotes-less entry
//! against the raw pinned schema alone would still pass -- that would
//! satisfy the AC's letter while missing its point, since this host never
//! ships `packages` and a remotes-less entry is one with no endpoint left
//! to publish. The test suite's own validation therefore layers the one
//! rule mcphost's listing actually needs (`required: ["remotes"]`) on top
//! of the pinned schema via `allOf`, still built from
//! `registry/schema.json`, not a second hand-maintained schema file.

use serde_json::{Value, json};

fn load_json(rel: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

/// The pinned schema, plus this host's own "a listing must name a remote"
/// rule -- see this file's own module doc comment for why that can't be
/// the bare pinned schema alone. The pinned schema's own top-level
/// `$ref: "#/definitions/ServerDetail"` is carried over unwrapped (its
/// `definitions` lifted to this composed document's own root) rather than
/// nested inside the new `allOf` -- a nested `$ref` would resolve against
/// the WRAPPER's root instead of the original schema's, and `definitions`
/// would no longer be there to find.
fn validator_requiring_remotes() -> jsonschema::Validator {
    let schema = load_json(mcphost::registry_manifest::SCHEMA_PATH);
    let composed = json!({
        "definitions": schema["definitions"],
        "allOf": [
            {"$ref": "#/definitions/ServerDetail"},
            {"required": ["remotes"]},
        ],
    });
    jsonschema::validator_for(&composed).expect("compile registry/schema.json + required:remotes")
}

#[test]
fn committed_server_json_validates_against_the_pinned_schema() {
    let validator = validator_requiring_remotes();
    let entry = load_json(mcphost::registry_manifest::MANIFEST_PATH);
    if let Some(err) = validator.iter_errors(&entry).next() {
        panic!("registry/server.json failed the pinned schema: {err} at {}", err.instance_path);
    }
}

#[test]
fn an_entry_missing_remotes_fails_the_schema() {
    let validator = validator_requiring_remotes();
    let mut entry = load_json(mcphost::registry_manifest::MANIFEST_PATH);
    entry
        .as_object_mut()
        .expect("server.json is an object")
        .remove("remotes");

    assert!(
        validator.iter_errors(&entry).next().is_some(),
        "an entry missing `remotes` must fail schema validation: {entry:?}"
    );
}
