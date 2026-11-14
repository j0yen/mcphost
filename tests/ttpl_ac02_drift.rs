//! contract: reads registry handler::host_tool_descriptors
//! PRD-mcphost-tenant-templates
//! AC2 -- Given `templates/*.toml`, When `cargo test ttpl_drift` runs, Then
//! every kind, spec field, demo tool and `next` verb resolves against the
//! registry and `host_tools`, and a fixture template with `kind = "webhook"`
//! fails naming the file and field.

use std::path::Path;

use mcphost::kinds::production_registry;
use mcphost::templates;

fn verbs() -> Vec<String> {
    mcphost::handler::host_tool_descriptors(&production_registry()).iter().map(|t| t.name.to_string()).collect()
}

#[test]
fn ttpl_drift_every_template_file_resolves_against_the_registry() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("templates");
    let kinds = production_registry();
    let verbs = verbs();
    let mut files: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("templates/") {
        let path = entry.expect("entry").path();
        if path.extension().is_some_and(|e| e == "toml") {
            let name = path.file_name().expect("name").to_string_lossy().to_string();
            let source = std::fs::read_to_string(&path).expect("read template");
            let problems = templates::drift(&format!("templates/{name}"), &source, &kinds, &verbs);
            assert!(problems.is_empty(), "{}", problems.join("\n"));
            files.push(name.trim_end_matches(".toml").to_string());
        }
    }
    files.sort();
    let mut shipped: Vec<String> = templates::SOURCES.iter().map(|(n, _)| n.to_string()).collect();
    shipped.sort();
    assert_eq!(files, shipped, "templates/*.toml and templates::SOURCES list the same files");
    assert_eq!(templates::all().len(), shipped.len(), "every shipped template parses");
}

#[test]
fn ttpl_drift_a_webhook_kind_fails_naming_the_file_and_field() {
    let fixture = r#"
title = "Bad"
one_line = "A template naming an alias as its tool kind."
plan_fit = "free"

[[tools]]
name = "inbox"
kind = "webhook"
[tools.spec]
program = "."

[demo]
call = { tool = "host.tool_call", args = { name = "inbox" } }
expect = { ok = true }
"#;
    let problems = templates::drift("templates/bad-webhook.toml", fixture, &production_registry(), &verbs());
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("templates/bad-webhook.toml"), "{problems:?}");
    assert!(problems[0].contains("tools[0].kind"), "{problems:?}");
    assert!(problems[0].contains("webhook"), "{problems:?}");
}

#[test]
fn ttpl_drift_names_an_unknown_spec_field_demo_tool_and_next_verb() {
    let fixture = r#"
title = "Bad"
one_line = "Fields that do not resolve."
plan_fit = "free"

[[tools]]
name = "job"
kind = "jq"
[tools.spec]
program = "."
programme = "."

[demo]
call = { tool = "host.no_such_verb", args = {} }
expect = { ok = true }

[[next]]
tool = "host.also_missing"
note = "x"
"#;
    let problems = templates::drift("templates/bad.toml", fixture, &production_registry(), &verbs()).join("\n");
    assert!(problems.contains("tools[0].spec.programme"), "{problems}");
    assert!(problems.contains("demo.call.tool: 'host.no_such_verb'"), "{problems}");
    assert!(problems.contains("next[0].tool: 'host.also_missing'"), "{problems}");
}
