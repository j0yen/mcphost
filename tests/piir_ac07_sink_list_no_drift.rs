//! PRD-mcphost-pii-tagging-and-log-redaction-v2
//! AC7 -- Given the `LogSink` enum, When the no-drift test runs, Then the sink
//! list in the generated contract equals the `Display` names of every enum
//! variant, no sink is listed twice, and a variant added without regenerating
//! the contract fails the test.

use mcphost::api_contract::dump_contract;
use mcphost::kinds::KindRegistry;
use mcphost::logsink::LogSink;
use regex::Regex;
use serde_json::Value;

/// The variant names of `pub enum LogSink { .. }` in `source`, in declaration order.
fn enum_variants(source: &str) -> Vec<String> {
    let start = source.find("pub enum LogSink {").expect("the enum");
    let body = &source[start..];
    let end = body.find("\n}\n").expect("the enum's closing brace");
    let variant = Regex::new(r"^    ([A-Z][A-Za-z0-9]*),$").expect("regex");
    body[..end].lines().filter_map(|l| variant.captures(l).map(|c| c[1].to_string())).collect()
}

fn contract_sinks(contract: &Value) -> Vec<String> {
    contract["log_sinks"]
        .as_array()
        .unwrap_or_else(|| panic!("the contract carries log_sinks: {}", contract["log_sinks"]))
        .iter()
        .map(|v| v.as_str().expect("a sink name").to_string())
        .collect()
}

/// What is wrong between the enum as written in `source` and a contract's sink list.
fn drift(source: &str, contract: &Value) -> Vec<String> {
    let variants = enum_variants(source);
    let listed = contract_sinks(contract);
    let display: Vec<String> = LogSink::ALL.iter().map(ToString::to_string).collect();
    let mut problems = Vec::new();
    if listed != display {
        problems.push(format!("contract log_sinks {listed:?} != Display names {display:?}: run `mcphost contract dump`"));
    }
    let mut sorted = variants.clone();
    sorted.sort();
    let mut sorted_display = display.clone();
    sorted_display.sort();
    if sorted != sorted_display {
        problems.push(format!("enum variants {variants:?} != Display names of LogSink::ALL {display:?}"));
    }
    let mut seen = std::collections::BTreeSet::new();
    for name in &listed {
        if !seen.insert(name) {
            problems.push(format!("sink {name} is listed twice"));
        }
    }
    problems
}

fn logsink_source() -> String {
    std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/logsink.rs")).expect("src/logsink.rs")
}

#[test]
fn piir_ac07_contract_sinks_equal_the_enum() {
    let source = logsink_source();
    assert!(enum_variants(&source).len() >= 5, "the parser sees the enum: {:?}", enum_variants(&source));

    // The generated contract and the committed one both carry the list.
    let generated = dump_contract(&KindRegistry::with_builtin());
    let committed: Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/contracts/host-tools.v1.json")).expect("committed contract"),
    )
    .expect("contract parses");
    assert_eq!(drift(&source, &generated), Vec::<String>::new(), "generated contract");
    assert_eq!(drift(&source, &committed), Vec::<String>::new(), "committed contract");
    assert_eq!(contract_sinks(&committed), LogSink::names());
    println!("piir_ac07: log_sinks = {:?}", contract_sinks(&committed));
}

#[test]
fn piir_ac07_a_variant_added_without_regenerating_fails() {
    let source = logsink_source();
    let committed: Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/contracts/host-tools.v1.json")).expect("committed contract"),
    )
    .expect("contract parses");
    // A variant added to the enum and nowhere else: `ALL` and the contract are stale.
    let drifted = source.replacen("pub enum LogSink {", "pub enum LogSink {\n    Planted,", 1);
    let problems = drift(&drifted, &committed);
    assert!(problems.iter().any(|p| p.contains("Planted")), "the planted variant is named: {problems:?}");

    // A contract that lists a sink twice, or one that was not regenerated, fails too.
    let mut doubled = committed.clone();
    doubled["log_sinks"].as_array_mut().expect("array").push(Value::String("ToolLog".to_string()));
    assert!(drift(&source, &doubled).iter().any(|p| p.contains("listed twice")), "{:?}", drift(&source, &doubled));
    let mut stale = committed;
    stale["log_sinks"].as_array_mut().expect("array").pop();
    assert!(!drift(&source, &stale).is_empty(), "a contract missing a sink fails");
}
