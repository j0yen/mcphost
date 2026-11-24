//! contract: reads registry handler::host_tool_descriptors
//! PRD-mcphost-plugin-skill-generated-from-registry
//! AC3 (P0) -- Given a new `Tool::new("host.example")` added to `host_tools`,
//! When `cargo test` runs, Then the drift test fails until `skill-gen
//! --write` regenerates and the new verb appears under its family.

use mcphost::kinds::KindRegistry;
use mcphost::skill_gen;
use std::path::PathBuf;

fn read(rel: &str) -> String {
    std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)).unwrap()
}

/// Every section but `devloop`, which needs the binary's clap definition: that
/// one is compared by `skgn_ac02` / `skgn_ac06` through the binary itself.
fn render_in_memory(plugin: &str, kinds: &KindRegistry, s: &str) -> Result<String, String> {
    if s == "devloop" {
        return skill_gen::extract_section(plugin, s).ok_or_else(|| "no devloop block".to_string());
    }
    skill_gen::render_section(s, kinds, None)
}

/// The drift test: the in-memory render of the plugin skill equals the
/// checked-in plugin file, and `www/skill.md` equals the plugin file.
#[test]
fn skgn_ac03_drift_test() {
    let kinds = KindRegistry::with_builtin();
    let plugin = read(skill_gen::PLUGIN_SKILL_PATH);
    let rendered = skill_gen::render_file(&plugin, skill_gen::SECTIONS, &|s| render_in_memory(&plugin, &kinds, s)).unwrap();
    if let Some((line, text)) = skill_gen::first_difference(&plugin, &rendered) {
        panic!("{} is stale at line {line}: {text} -- run `mcphost skill-gen --write`", skill_gen::PLUGIN_SKILL_PATH);
    }
    assert_eq!(read(skill_gen::WWW_SKILL_PATH), plugin, "www/skill.md must be byte-equal to the plugin skill");
}

/// A verb added to the registry makes the drift comparison fail, and the
/// regenerated text carries the verb under its family.
#[test]
fn skgn_ac03_new_verb_fails_drift_until_regenerated() {
    let kinds = KindRegistry::with_builtin();
    let plugin = read(skill_gen::PLUGIN_SKILL_PATH);
    let mut names = skill_gen::registry_tool_names(&kinds);
    names.push("host.example".to_string());
    let starters = mcphost::handler::starter_tool_names(&kinds);
    let render = |s: &str| match s {
        "verbs" => Ok(skill_gen::render_verbs(&names, &starters)),
        other => render_in_memory(&plugin, &kinds, other),
    };
    let regenerated = skill_gen::render_file(&plugin, skill_gen::SECTIONS, &render).unwrap();
    let (line, _) = skill_gen::first_difference(&plugin, &regenerated).expect("a new verb must make the checked-in skill stale");
    assert!(line > 1);
    let family_line = regenerated
        .lines()
        .find(|l| l.starts_with("- `host.*` "))
        .expect("the host.* family line");
    assert!(family_line.contains("example"), "new verb under its family: {family_line}");
    assert!(!plugin.lines().any(|l| l.starts_with("- `host.*` ") && l.contains("example")));
    // Regenerating again from the regenerated text is a fixed point.
    let again = skill_gen::render_file(&regenerated, skill_gen::SECTIONS, &render).unwrap();
    assert_eq!(again, regenerated);
}
