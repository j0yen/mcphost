//! contract: reads registry kinds::python::BRIDGE_MODULES
//! PRD-mcphost-tool-reach-manifest-v2
//! AC6 (P0) -- Given `BRIDGE_MODULES` or `BRIDGE_ATTRS` gains an entry with no
//! `REACH_CATEGORIES` rule, When the drift test runs, Then it fails naming the
//! entry; Given both sets are covered, Then it passes.
//!
//! The drift test is `live_bridge_sets_are_covered`. The failure half is shown
//! on a copy of the live sets with one extra module and one extra attribute:
//! the same check returns exactly those two names.

use mcphost::kinds::python::{BRIDGE_ATTRS, BRIDGE_MODULES};
use mcphost::reach::{REACH_CATEGORIES, STATE_VERBS, uncovered_bridge_names, uncovered_names, uncovered_state_verbs};

#[test]
fn live_bridge_sets_are_covered() {
    let missing = uncovered_bridge_names();
    assert!(missing.is_empty(), "BRIDGE_MODULES / BRIDGE_ATTRS entries with no REACH_CATEGORIES rule: {missing:?}");
}

#[test]
fn a_new_bridge_entry_without_a_rule_fails_and_is_named() {
    let mut modules: Vec<&str> = BRIDGE_MODULES.iter().map(|m| m.name).collect();
    let mut attrs: Vec<&str> = BRIDGE_ATTRS.to_vec();
    assert!(uncovered_names(&modules, &attrs).is_empty(), "the live sets pass before the extra entries");

    modules.push("telemetry");
    assert_eq!(uncovered_names(&modules, &attrs), ["telemetry"], "a new module is named");
    attrs.push("mcphost.snapshot(label) -- a new attribute-style bridge function");
    assert_eq!(uncovered_names(&modules, &attrs), ["telemetry", "snapshot"], "a new attribute is named");
}

#[test]
fn every_rule_names_a_real_bridge_entry() {
    let modules: Vec<&str> = BRIDGE_MODULES.iter().map(|m| m.name).collect();
    for (name, _) in REACH_CATEGORIES {
        let is_attr = BRIDGE_ATTRS.iter().any(|a| a.starts_with(&format!("mcphost.{name}(")));
        assert!(modules.contains(name) || is_attr, "REACH_CATEGORIES has a rule for {name}, which is no bridge entry");
    }
}

#[test]
fn every_state_verb_has_a_list_and_every_list_names_a_verb() {
    let state = BRIDGE_MODULES.iter().find(|m| m.name == "state").expect("the state module");
    assert!(uncovered_state_verbs(state.signatures).is_empty(), "{:?}", uncovered_state_verbs(state.signatures));
    for (verb, _) in STATE_VERBS {
        assert!(
            state.signatures.iter().any(|s| s.starts_with(&format!("mcphost.state.{verb}("))),
            "STATE_VERBS names {verb}, which mcphost.state does not have"
        );
    }
    assert_eq!(uncovered_state_verbs(&["mcphost.state.touch(key) -- a new verb"]), ["touch"]);
}
