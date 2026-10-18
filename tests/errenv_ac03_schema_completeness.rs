//! PRD-mcphost-error-envelope-schema-contract
//! AC3 (P0) -- Given a verb whose registered output schema lacks the shared
//! error branch, When `cargo test` runs, Then a schema-completeness test fails
//! naming that verb, and the check enumerates the registry rather than a hand
//! list.

use mcphost::errors::{ERROR_ENVELOPE_DEF, error_envelope_schema};
use mcphost::output_schemas::{ERROR_BRANCH_KEY, error_branch_of, output_schema_for, registered_tools};
use serde_json::Value;

/// Every verb in the registry whose schema lacks the shared branch, by name.
fn incomplete(schemas: impl Iterator<Item = (String, serde_json::Map<String, Value>)>) -> Vec<String> {
    schemas.filter_map(|(tool, schema)| error_branch_of(&tool, &schema).err()).collect()
}

#[test]
fn errenv_ac03() {
    let tools = registered_tools();
    assert!(tools.len() > 100, "registry enumeration looks broken: {} verbs", tools.len());
    let missing = incomplete(tools.iter().map(|t| (t.to_string(), (*output_schema_for(t).expect("registered")).clone())));
    assert!(missing.is_empty(), "verbs lacking the shared error branch:\n{}", missing.join("\n"));
    // One definition, referenced: every verb's `$defs` entry is the generated schema.
    for tool in &tools {
        let schema = output_schema_for(tool).expect("registered");
        assert_eq!(schema["$defs"][ERROR_ENVELOPE_DEF], error_envelope_schema(), "{tool}: restated or drifted envelope");
    }
}

#[test]
fn errenv_ac03_a_schema_without_the_branch_is_named() {
    let mut stripped = (*output_schema_for("host.whoami").expect("registered")).clone();
    stripped.remove(ERROR_BRANCH_KEY);
    let missing = incomplete(std::iter::once(("host.whoami".to_string(), stripped)));
    assert_eq!(missing.len(), 1);
    assert!(missing[0].starts_with("host.whoami:"), "{missing:?}");
}
