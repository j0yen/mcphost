//! AC6 (PRD-mcphost-plugin-skill-generated-from-tools-list) -- Given a binary
//! built without the `dev`/`push`/`repo` subcommands, When `skill-gen`
//! renders `devloop`, Then the section says `no local verbs in this build`
//! and names no absent verb.

use clap::Command;
use mcphost::skill_gen::{self, Section};

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect()
}

#[test]
fn skgn_ac06() {
    // A binary with none of the dev-loop commands.
    let bare = Command::new("mcphost")
        .subcommand(Command::new("serve").about("Run the server."))
        .subcommand(Command::new("version").about("Print the version."));
    let section = skill_gen::render_devloop(&bare);
    assert!(section.starts_with(&Section::Devloop.start_marker()));
    assert!(section.ends_with(&Section::Devloop.end_marker()));
    assert!(section.contains("no local verbs in this build"), "{section}");
    let found = words(&section);
    for absent in skill_gen::DEVLOOP_COMMANDS {
        assert!(!found.iter().any(|w| w == absent), "section names absent verb `{absent}`: {section}");
    }
    assert!(!section.contains("`mcphost serve`"), "only dev-loop commands are listed");

    // A binary that has `dev` (and a hidden `push`) lists exactly what it has.
    let with_dev = Command::new("mcphost")
        .subcommand(
            Command::new("dev")
                .about("Run a local loop. Second sentence.")
                .subcommand(Command::new("watch").about("Rebuild on change.")),
        )
        .subcommand(Command::new("push").about("Push a tool.").hide(true));
    let section = skill_gen::render_devloop(&with_dev);
    assert!(!section.contains("no local verbs in this build"), "{section}");
    assert!(section.contains("- `mcphost dev` -- Run a local loop.\n"), "{section}");
    assert!(section.contains("  - `mcphost dev watch` -- Rebuild on change.\n"), "{section}");
    assert!(!words(&section).iter().any(|w| w == "push" || w == "repo" || w == "tool"), "{section}");

    // The real binary today has none of them, and the checked-in files say so
    // (the drift test in skgn_ac03 runs the binary against these very files).
    for path in [skill_gen::SKILL_PATH, skill_gen::WWW_SKILL_PATH] {
        let content = std::fs::read_to_string(path).expect("read skill");
        assert!(content.contains(&Section::Devloop.start_marker()), "{path} carries the devloop markers");
    }
}
