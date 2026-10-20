//! PRD-mcphost-onboarding-dead-ends-zero-b AC2 (P0) -- Given the source tree,
//! When the grep test runs, Then `"next_call"` is written into an error
//! envelope in exactly one place outside the table, and every `tool` the table
//! emits has its `args` keys inside that tool's input schema.

//! contract: reads registry handler::host_tool_descriptors

use mcphost::error_registry::ERROR_CODES;
use mcphost::errors::{AppError, all_variants_for_test};
use mcphost::handler::host_tool_descriptors;
use mcphost::kinds::production_registry;
use mcphost::next_call::ErrorCtx;
use serde_json::Value;
use std::path::Path;

fn rs_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read_dir") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            rs_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn ndeadb_ac02_next_call_is_inserted_in_exactly_one_place() {
    let mut files = Vec::new();
    rs_files(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
    let mut sites = Vec::new();
    for file in files {
        if file.file_name().is_some_and(|n| n == "next_call.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&file).expect("read");
        for (i, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            if line.contains(r#"insert("next_call""#) || line.contains(r#"insert("next_call".to_string()"#) {
                sites.push(format!("{}:{}", file.display(), i + 1));
            }
        }
    }
    assert_eq!(sites.len(), 1, "next_call must be inserted at one envelope point, found {sites:?}");
    assert!(sites[0].contains("errors.rs"), "{sites:?}");
}

#[test]
fn ndeadb_ac02_every_emitted_tool_has_its_args_in_the_schema() {
    let registry = production_registry();
    let descriptors = host_tool_descriptors(&registry);
    let kind = registry.names().first().map(|k| k.to_string());

    let mut errors = all_variants_for_test();
    for row in ERROR_CODES.iter() {
        errors.push(AppError::Structured { code: row.code, message: String::new(), data: Value::Null });
    }
    for code in ["args_invalid", "tool_exception", "upstream_status", "upstream_auth", "host_not_allowed", "plan_limit"] {
        errors.push(AppError::Structured { code: Box::leak(code.to_string().into_boxed_str()), message: String::new(), data: Value::Null });
    }
    let ctxs = [
        ErrorCtx::default(),
        ErrorCtx {
            tool: Some("mine".into()),
            kind,
            argument: Some("n".into()),
            got: Some("4.78".into()),
            expected_type: Some("integer".into()),
            secrets: vec!["API_TOKEN".into()],
            plan_ceiling: Some("tools_max".into()),
            retry_after_s: Some(7),
        },
    ];
    let mut checked = 0;
    for ctx in &ctxs {
        for err in &errors {
            let next = err.next_call(ctx);
            let canonical = mcphost::tool_aliases::resolve(next.tool).unwrap_or(next.tool);
            let tool = descriptors
                .iter()
                .find(|t| t.name.as_ref() == canonical)
                .unwrap_or_else(|| panic!("{} -> {} is not a listed tool", err.code(), next.tool));
            let props = tool.input_schema.get("properties").and_then(Value::as_object);
            for key in next.args.as_object().expect("args is an object").keys() {
                assert!(
                    props.is_some_and(|p| p.contains_key(key)),
                    "{} -> {} arg '{key}' is not in its input schema",
                    err.code(),
                    next.tool
                );
            }
            checked += 1;
        }
    }
    assert!(checked > 80, "checked only {checked}");
}
