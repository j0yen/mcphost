//! PRD-mcphost-plugin-skill-generated-from-registry
//! AC4 (P0) -- Given `ERROR_CODES` gains an entry, When `cargo test` runs,
//! Then the drift test fails until the skill is regenerated, and a remedy
//! that names a verb absent from `host_tools` fails the remedy test naming
//! the code.

use mcphost::error_registry::{ERROR_CODES, ErrorClass, ErrorEntry, Family, row};
use mcphost::kinds::KindRegistry;
use mcphost::skill_gen;
use std::path::PathBuf;

fn plugin() -> String {
    std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(skill_gen::PLUGIN_SKILL_PATH)).unwrap()
}

#[test]
fn skgn_ac04_every_code_and_remedy_is_in_the_checked_in_skill() {
    let text = plugin();
    for e in ERROR_CODES.iter() {
        let line = format!("- `{}` ({}): {}", e.code, e.class.as_str(), e.remedy);
        assert!(text.lines().any(|l| l == line), "skill lacks the line for {}: {line}", e.code);
    }
}

#[test]
fn skgn_ac04_new_error_code_fails_drift_until_regenerated() {
    let kinds = KindRegistry::with_builtin();
    let text = plugin();
    let extra: ErrorEntry = row("zzz_skgn_new_code", Family::Args, ErrorClass::AgentFault, false, "Call host.whoami and retry.");
    let render = |s: &str| match s {
        "errors" => Ok(skill_gen::render_errors_from(ERROR_CODES.iter().chain(std::iter::once(&extra)))),
        "devloop" => skill_gen::extract_section(&text, "devloop").ok_or_else(|| "no devloop block".to_string()),
        other => skill_gen::render_section(other, &kinds, None),
    };
    let regenerated = skill_gen::render_file(&text, skill_gen::SECTIONS, &render).unwrap();
    assert!(skill_gen::first_difference(&text, &regenerated).is_some(), "a new code must make the skill stale");
    assert!(regenerated.contains("- `zzz_skgn_new_code` (agent_fault): Call host.whoami and retry."));
}

#[test]
fn skgn_ac04_remedy_names_only_verbs_the_host_serves() {
    let kinds = KindRegistry::with_builtin();
    let names = skill_gen::registry_tool_names(&kinds);
    let bad = skill_gen::unknown_remedy_verbs(&names);
    assert!(bad.is_empty(), "remedies name verbs absent from host_tools: {bad:?}");

    let ghost: ErrorEntry = row("zzz_skgn_ghost", Family::Args, ErrorClass::AgentFault, false, "Run host.ghost.verb first, then host.whoami.");
    let bad = skill_gen::unknown_remedy_verbs_in(std::iter::once(&ghost), &names);
    assert_eq!(bad, vec![("zzz_skgn_ghost".to_string(), "host.ghost.verb".to_string())], "must name the code and the verb");
}
