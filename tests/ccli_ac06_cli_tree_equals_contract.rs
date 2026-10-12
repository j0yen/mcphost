//! AC6 (PRD-mcphost-call-cli-from-contract) -- Given the contract and the CLI
//! tree, When the drift test compares subcommand names, flag names and help
//! text, Then all three sets are equal; one tool removed from the embedded
//! contract fails the test naming it.
//!
//! The expectation is computed here from the raw contract JSON with its own
//! small rules (not by calling `call_cli`'s parser), so a bug shared by the
//! tree builder and a re-use of it cannot hide.

use std::collections::{BTreeMap, BTreeSet};

use clap::Command;
use mcphost::call_cli::{self, Contract, EMBEDDED_CONTRACT};
use mcphost::tool_aliases::TOOL_ALIASES;
use serde_json::Value;

/// Flags `mcphost call` owns; present on a subcommand only as propagated globals.
const OWN_FLAGS: &[&str] = &["url", "key-file", "json", "no-skew-check", "help"];

struct Expected {
    about: String,
    /// flag -> help
    flags: BTreeMap<String, String>,
}

fn first_sentence(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut = flat.split(". ").next().unwrap_or("").trim_end_matches('.').to_string();
    if cut.chars().count() <= 120 {
        cut
    } else {
        format!("{}…", cut.chars().take(119).collect::<String>())
    }
}

fn is_plain(schema: &Value) -> bool {
    let types: Vec<&str> = match &schema["type"] {
        Value::String(t) => vec![t.as_str()],
        Value::Array(ts) => ts.iter().filter_map(Value::as_str).collect(),
        _ => vec![],
    };
    if types.contains(&"array") {
        return matches!(schema["items"]["type"].as_str(), Some("string" | "integer"));
    }
    !types.contains(&"object") && types.iter().any(|t| ["string", "integer", "number", "boolean"].contains(t))
}

fn expected_from(contract_json: &Value) -> BTreeMap<String, Expected> {
    let aliases: BTreeSet<&str> = TOOL_ALIASES.iter().map(|a| a.alias).collect();
    let mut out = BTreeMap::new();
    for tool in contract_json["tools"].as_array().expect("tools array") {
        let name = tool["name"].as_str().expect("tool name");
        if !name.contains('.') || aliases.contains(name) {
            continue;
        }
        let mut flags = BTreeMap::new();
        for (prop, schema) in tool["input_schema"]["properties"].as_object().into_iter().flatten() {
            if prop == "tenant_key" {
                continue;
            }
            let mut flag = prop.replace('_', "-");
            if !is_plain(schema) {
                flag.push_str("-json");
            }
            // Same collision rule as call_cli::build_command: a property whose
            // flag name is one of `mcphost call`'s own flags is exposed as
            // `--<name>-arg` (e.g. host.blob.put's `url`).
            if OWN_FLAGS.contains(&flag.as_str()) {
                flag.push_str("-arg");
            }
            flags.insert(flag, first_sentence(schema["description"].as_str().unwrap_or("")));
        }
        out.insert(
            name.to_string(),
            Expected { about: first_sentence(tool["description"].as_str().unwrap_or("")), flags },
        );
    }
    out
}

/// Every difference between `cmd` and the contract, each naming the tool.
fn drift(cmd: &Command, contract_json: &Value) -> Vec<String> {
    let expected = expected_from(contract_json);
    let mut problems = Vec::new();
    let actual: BTreeMap<&str, &Command> = cmd.get_subcommands().map(|s| (s.get_name(), s)).collect();
    for name in expected.keys() {
        if !actual.contains_key(name.as_str()) {
            problems.push(format!("tool {name} is in the contract but not in the CLI"));
        }
    }
    for name in actual.keys() {
        if !expected.contains_key(*name) {
            problems.push(format!("tool {name} is in the CLI but not in the contract"));
        }
    }
    for (name, want) in &expected {
        let Some(sub) = actual.get(name.as_str()) else { continue };
        let about = sub.get_about().map(ToString::to_string).unwrap_or_default();
        if about != want.about {
            problems.push(format!("tool {name}: help {about:?} != {:?}", want.about));
        }
        let got: BTreeMap<String, String> = sub
            .get_arguments()
            .filter_map(|a| a.get_long().map(|l| (l, a)))
            .filter(|(l, _)| !OWN_FLAGS.contains(l))
            .map(|(l, a)| (l.to_string(), a.get_help().map(ToString::to_string).unwrap_or_default()))
            .collect();
        for flag in want.flags.keys().filter(|f| !got.contains_key(*f)) {
            problems.push(format!("tool {name}: flag --{flag} is in the contract but not in the CLI"));
        }
        for flag in got.keys().filter(|f| !want.flags.contains_key(*f)) {
            problems.push(format!("tool {name}: flag --{flag} is in the CLI but not in the contract"));
        }
        for (flag, help) in &got {
            if let Some(want_help) = want.flags.get(flag)
                && help != want_help
            {
                problems.push(format!("tool {name}: --{flag} help {help:?} != {want_help:?}"));
            }
        }
    }
    problems
}

fn embedded_json() -> Value {
    serde_json::from_slice(EMBEDDED_CONTRACT).expect("embedded contract is JSON")
}

#[test]
fn subcommands_flags_and_help_equal_the_contract() {
    let cmd = call_cli::build_command(call_cli::contract());
    let problems = drift(&cmd, &embedded_json());
    assert!(problems.is_empty(), "CLI tree drifted from the contract:\n{}", problems.join("\n"));
}

#[test]
fn the_embedded_copy_is_the_committed_contract_file() {
    let on_disk = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/contracts/host-tools.v1.json")).expect("read contract");
    assert_eq!(on_disk, EMBEDDED_CONTRACT);
}

#[test]
fn one_tool_removed_from_the_contract_fails_naming_it() {
    // The tree is built from the full contract; the contract it is compared
    // with has `host.redeem` removed -- as if a tool left `host_tools`.
    let cmd = call_cli::build_command(call_cli::contract());
    let mut shrunk = embedded_json();
    let tools = shrunk["tools"].as_array_mut().unwrap();
    tools.retain(|t| t["name"] != "host.redeem");
    let problems = drift(&cmd, &shrunk);
    assert!(
        problems.iter().any(|p| p.contains("host.redeem") && p.contains("not in the contract")),
        "expected the drift to name host.redeem, got {problems:?}"
    );

    // And the other direction: a tool added to the contract the tree lacks.
    let mut bytes_without = embedded_json();
    bytes_without["tools"].as_array_mut().unwrap().retain(|t| t["name"] != "host.redeem");
    let stale_cmd = call_cli::build_command(&Contract::parse(&serde_json::to_vec(&bytes_without).unwrap()).unwrap());
    let problems = drift(&stale_cmd, &embedded_json());
    assert!(
        problems.iter().any(|p| p.contains("host.redeem") && p.contains("not in the CLI")),
        "expected the drift to name host.redeem, got {problems:?}"
    );
}
