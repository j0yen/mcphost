//! AC3 (PRD-mcphost-plugin-skill-generated-from-tools-list) -- Given a new
//! `Tool::new("host.example")` added to `host_tools`, When `cargo test` runs,
//! Then the drift test fails until `skill-gen --write` regenerates and the new
//! verb appears under its family.
//!
//! The drift test is the first half: every section, rendered in memory from
//! the registry, must be byte-equal to both checked-in files. The second half
//! adds a `host.example` row to the registry's verbs (what a new
//! `Tool::new("host.example")` does) and shows the checked-in files then
//! disagree, and that regenerating is what puts the verb under its family.

use mcphost::kinds::KindRegistry;
use mcphost::skill_gen::{self, Section, Sources, VerbRow};

fn checked_in() -> Vec<(&'static str, String)> {
    [skill_gen::SKILL_PATH, skill_gen::WWW_SKILL_PATH]
        .iter()
        .map(|p| (*p, std::fs::read_to_string(p).unwrap_or_else(|e| panic!("read {p}: {e}"))))
        .collect()
}

#[test]
fn skgn_ac03() {
    let sources = Sources::from_registry(&KindRegistry::with_builtin());

    // The drift test: both checked-in files equal the in-memory render.
    for (path, content) in checked_in() {
        let rendered = skill_gen::render_file(&content, Section::LIB, &sources)
            .unwrap_or_else(|why| panic!("{path}: {why}"));
        assert_eq!(
            skill_gen::first_diff_line(&content, &rendered),
            None,
            "{path} drifted from `mcphost skill-gen` (run `mcphost skill-gen --write`)"
        );
    }

    // `devloop` renders from the binary's clap tree, which a test cannot
    // build in memory: the binary checks it (and every other section).
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .arg("skill-gen")
        .output()
        .expect("run mcphost skill-gen");
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));

    // A verb nobody has regenerated for.
    let mut with_example = Sources::from_registry(&KindRegistry::with_builtin());
    with_example.verbs.push(VerbRow {
        name: "host.example".to_string(),
        description: "A brand new verb. It has a second sentence.".to_string(),
        alias_of: None,
    });
    for (path, content) in checked_in() {
        let regenerated = skill_gen::render_file(&content, Section::LIB, &with_example)
            .unwrap_or_else(|why| panic!("{path}: {why}"));
        assert!(
            skill_gen::first_diff_line(&content, &regenerated).is_some(),
            "{path}: the drift test must fail while host.example is missing from the checked-in file"
        );
        assert!(!content.contains("`host.example`"), "{path}: precondition");

        // Regenerated, the verb sits under its family heading, first sentence only.
        let family = regenerated.find("#### `host.*`").expect("host.* family heading");
        let line = regenerated.find("- `host.example` -- A brand new verb.\n").expect("host.example bullet");
        let next_family = regenerated[family + 1..].find("\n#### ").map_or(regenerated.len(), |i| family + 1 + i);
        assert!(family < line && line < next_family, "{path}: host.example must sit under `host.*`");
        assert!(!regenerated.contains("It has a second sentence"), "{path}: first sentence only");
    }
}
