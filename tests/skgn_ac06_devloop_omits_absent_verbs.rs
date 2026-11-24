//! PRD-mcphost-plugin-skill-generated-from-registry
//! AC6 (P0) -- Given a binary built without the `dev` or `push`
//! subcommands, When `skill-gen` renders `devloop`, Then the section says
//! `no local verbs in this build` for the absent verbs and names none of
//! them.

use clap::{Arg, Command};
use mcphost::skill_gen::render_devloop;

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

fn full_cli() -> Command {
    Command::new("mcphost")
        .subcommand(Command::new("tool").about("a tool as a directory").subcommand(Command::new("lint").about("Lint a directory").arg(Arg::new("dir").required(true))))
        .subcommand(Command::new("dev").about("run a tool locally").arg(Arg::new("dir").required(true)).arg(Arg::new("port").long("port").num_args(1)))
        .subcommand(Command::new("push").about("publish a directory").arg(Arg::new("dir").required(true)))
}

#[test]
fn skgn_ac06_a_build_without_dev_and_push_names_neither() {
    let cli = Command::new("mcphost").subcommand(Command::new("tool").about("a tool as a directory").subcommand(Command::new("lint").about("Lint a directory").arg(Arg::new("dir").required(true))));
    let section = render_devloop(&cli);
    assert!(section.contains("no local verbs in this build"), "{section}");
    let w = words(&section);
    for absent in ["dev", "push"] {
        assert!(!w.iter().any(|x| x == absent), "absent verb `{absent}` is named: {section}");
    }
    assert!(section.contains("mcphost tool lint <DIR>"), "the present verb is rendered from clap: {section}");
}

#[test]
fn skgn_ac06_a_build_with_no_local_verbs_names_none() {
    let section = render_devloop(&Command::new("mcphost").subcommand(Command::new("serve")));
    assert!(section.contains("no local verbs in this build"));
    let w = words(&section);
    for absent in ["tool", "dev", "push"] {
        assert!(!w.iter().any(|x| x == absent), "absent verb `{absent}` is named: {section}");
    }
}

#[test]
fn skgn_ac06_a_full_build_lists_all_three_and_no_absence_line() {
    let section = render_devloop(&full_cli());
    assert!(!section.contains("no local verbs in this build"), "{section}");
    assert!(section.contains("`mcphost dev <DIR> [--port <PORT>]`"), "{section}");
    assert!(section.contains("`mcphost push <DIR>`"), "{section}");
}
