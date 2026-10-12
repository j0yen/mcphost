//! AC1 (PRD-mcphost-call-cli-from-contract) -- Given the embedded contract,
//! When the clap tree is built, Then `Command::debug_assert` passes and
//! fewer than 5 canonical tools needed any override; the test prints the
//! override list and fails the premise at 5.

use std::collections::BTreeSet;

use mcphost::call_cli;

#[test]
fn clap_tree_builds_and_fewer_than_five_tools_need_an_override() {
    let contract = call_cli::contract();
    assert!(
        contract.tools.len() > 100,
        "expected the embedded contract's canonical tools, got {}",
        contract.tools.len()
    );

    let cmd = call_cli::build_command(contract);
    // Panics (failing the test) on a duplicate id, a clashing long flag or
    // any other malformed arg -- the falsifier's "tree clap rejects".
    cmd.clone().debug_assert();
    assert_eq!(
        cmd.get_subcommands().count(),
        contract.tools.len(),
        "one subcommand per canonical tool"
    );

    for line in &contract.overrides {
        println!("override: {line}");
    }
    let tools_overridden: BTreeSet<&str> = contract
        .overrides
        .iter()
        .filter_map(|line| line.split(':').next())
        .collect();
    println!(
        "{} of {} canonical tools needed an override",
        tools_overridden.len(),
        contract.tools.len()
    );
    assert!(
        tools_overridden.len() < 5,
        "premise failed: {} tools needed a hand override ({tools_overridden:?}) -- the table is not generated",
        tools_overridden.len()
    );
}
