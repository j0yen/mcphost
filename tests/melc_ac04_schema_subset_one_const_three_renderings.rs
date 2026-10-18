//! contract: reads docs/sandbox-api.md
//! PRD-mcphost-elicitation-with-ask-fallback
//! AC4 (P0) — Given `ELICIT_SCHEMA_SUBSET`, When the bridge docstring,
//! `docs/sandbox-api.md` and the rmcp mapping test are rendered, Then all
//! three list exactly the const's types and limits.

use mcphost::elicit::{
    DOCS_END, DOCS_START, ELICIT_RMCP_VARIANTS, ELICIT_SCHEMA_SUBSET, render_docs_block, render_subset,
    rmcp_variant_name, validate_elicit_schema,
};
use serde_json::{Value, json};
use std::io::Write;
use std::process::{Command, Stdio};

/// `help(mcphost.elicit)`'s text, read off the real module the sandbox registers.
fn bridge_docstring() -> String {
    let mut script = mcphost::kinds::python::runner_script_module_prelude().to_string();
    script.push_str("\nprint(sys.modules['mcphost.elicit'].__doc__)\n");
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn python3");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "python3 failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// One property of each type the mapping test builds, keyed by const type.
fn sample(ty: &str) -> Value {
    match ty {
        "enum" => json!({"type": "enum", "enum": ["a", "b"]}),
        other => json!({"type": other}),
    }
}

/// The mapping test's own rendering: types come from what it actually mapped.
fn mapped_rendering() -> String {
    let mut mapped = Vec::new();
    for ty in ELICIT_SCHEMA_SUBSET.types {
        let schema = validate_elicit_schema(&json!({"x": sample(ty)})).unwrap_or_else(|e| panic!("{ty}: {e:?}"));
        let rmcp = schema.to_rmcp().unwrap_or_else(|e| panic!("{ty}: {e:?}"));
        let def = rmcp.properties.get("x").expect("property x");
        let variant = rmcp_variant_name(def);
        let expected = ELICIT_RMCP_VARIANTS.iter().find(|(t, _)| t == ty).map(|(_, v)| *v);
        assert_eq!(Some(variant), expected, "type `{ty}` must map to exactly its rmcp variant");
        mapped.push(*ty);
    }
    let types: Vec<&str> = ELICIT_RMCP_VARIANTS.iter().map(|(t, _)| *t).collect();
    assert_eq!(types, ELICIT_SCHEMA_SUBSET.types, "the mapping table covers exactly the const's types");
    // Limits: the boundary value passes, one past it is refused.
    let props = |n: usize| -> Value { (0..n).map(|i| (format!("p{i}"), json!({"type": "string"}))).collect::<serde_json::Map<_, _>>().into() };
    assert!(validate_elicit_schema(&props(ELICIT_SCHEMA_SUBSET.max_properties)).is_ok());
    assert!(validate_elicit_schema(&props(ELICIT_SCHEMA_SUBSET.max_properties + 1)).is_err());
    let len = |n: usize| json!({"x": {"type": "string", "maxLength": n}});
    assert!(validate_elicit_schema(&len(ELICIT_SCHEMA_SUBSET.string_max_length)).is_ok());
    assert!(validate_elicit_schema(&len(ELICIT_SCHEMA_SUBSET.string_max_length + 1)).is_err());
    let values = |n: usize| json!({"x": {"enum": (0..n).map(|i| format!("v{i}")).collect::<Vec<_>>()}});
    assert!(validate_elicit_schema(&values(ELICIT_SCHEMA_SUBSET.enum_max_values)).is_ok());
    assert!(validate_elicit_schema(&values(ELICIT_SCHEMA_SUBSET.enum_max_values + 1)).is_err());
    format!(
        "types: {}; at most {} properties; string maxLength at most {}; enum at most {} values",
        mapped.join(", "),
        ELICIT_SCHEMA_SUBSET.max_properties,
        ELICIT_SCHEMA_SUBSET.string_max_length,
        ELICIT_SCHEMA_SUBSET.enum_max_values
    )
}

#[test]
fn bridge_docstring_docs_and_mapping_test_all_list_the_consts_types_and_limits() {
    let line = render_subset();
    for ty in ELICIT_SCHEMA_SUBSET.types {
        assert!(line.contains(ty), "{line}");
    }

    let docstring = bridge_docstring();
    assert!(docstring.contains(&line), "bridge docstring must carry `{line}`:\n{docstring}");

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let doc = std::fs::read_to_string(root.join("docs/sandbox-api.md")).expect("docs/sandbox-api.md");
    let start = doc.find(DOCS_START).expect("start marker");
    let end = doc.find(DOCS_END).expect("end marker") + DOCS_END.len();
    assert_eq!(&doc[start..end], render_docs_block(), "docs/sandbox-api.md block drifted from the const");

    assert_eq!(mapped_rendering(), line, "the rmcp mapping test must list the const's types and limits");
}
