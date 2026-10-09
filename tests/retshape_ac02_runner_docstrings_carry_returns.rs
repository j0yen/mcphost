//! PRD-mcphost-sandbox-return-shape-contract
//! AC2 (P0) -- Given the rendered runner script, When its docstrings are
//! re-parsed by the test, Then every function in `BRIDGE_MODULES` carries a
//! `Returns:` line whose keys equal the table's, and a function added to
//! `BRIDGE_MODULES` without a table row fails the test.

use mcphost::kinds::python::{
    BRIDGE_ATTRS, BRIDGE_MODULES, BRIDGE_RETURNS, BridgeModule, ReturnShape, bridge_functions_without_return_row,
    runner_script_module_prelude, signature_function,
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::process::{Command, Stdio};

/// Execs the rendered runner prelude under python3 and returns every named
/// function's own `__doc__` -- the text `help()` prints, read back off the
/// real function objects rather than from the Rust table.
fn docstrings(functions: &[&str]) -> BTreeMap<String, Option<String>> {
    let mut script = runner_script_module_prelude().to_string();
    script.push_str("\nimport json as _j\n_out = {}\n");
    for f in functions {
        let (module, name) = f.rsplit_once('.').unwrap();
        script.push_str(&format!(
            "_out[{f:?}] = getattr(sys.modules[{module:?}], {name:?}).__doc__\n"
        ));
    }
    script.push_str("print('DOCS ' + _j.dumps(_out))\n");
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn python3");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "python3 failed: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout.lines().find_map(|l| l.strip_prefix("DOCS ")).expect("DOCS line printed");
    let parsed: Value = serde_json::from_str(line).unwrap();
    parsed
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().map(str::to_string)))
        .collect()
}

/// The text after the first `Returns:` line of a docstring.
fn returns_line(doc: &str) -> Option<&str> {
    doc.lines().find_map(|l| l.trim().strip_prefix("Returns:")).map(str::trim)
}

/// `{"table": str, "rows": list}` -> {table, rows}; a plain description has no keys.
fn keys_in(returns: &str) -> BTreeSet<String> {
    let Some(body) = returns.strip_prefix('{') else { return BTreeSet::new() };
    body.split(',')
        .filter_map(|pair| pair.split(':').next())
        .map(|k| k.trim().trim_matches('"').to_string())
        .collect()
}

fn table_keys(function: &str) -> BTreeSet<String> {
    let row = BRIDGE_RETURNS.iter().find(|r| r.function == function).expect("table row");
    match row.shape {
        ReturnShape::Envelope(keys) => keys.iter().map(|(k, _)| k.to_string()).collect(),
        ReturnShape::Plain(_) => BTreeSet::new(),
    }
}

#[test]
fn every_bridge_function_docstring_has_a_returns_line_matching_the_table() {
    let functions: Vec<&str> = BRIDGE_MODULES
        .iter()
        .flat_map(|m| m.signatures.iter().copied())
        .chain(BRIDGE_ATTRS.iter().copied())
        .map(signature_function)
        .collect();
    assert!(functions.len() > 20, "signature parser broke: {functions:?}");
    let docs = docstrings(&functions);

    for f in &functions {
        let doc = docs[*f].as_deref().unwrap_or_else(|| panic!("{f} has no docstring"));
        let returns = returns_line(doc).unwrap_or_else(|| panic!("{f} docstring has no `Returns:` line:\n{doc}"));
        assert_eq!(keys_in(returns), table_keys(f), "{f}: Returns keys differ from the table: {returns}");
    }
}

#[test]
fn a_function_added_to_bridge_modules_without_a_table_row_fails() {
    assert!(bridge_functions_without_return_row(BRIDGE_MODULES, BRIDGE_RETURNS).is_empty());

    let fixture = BridgeModule {
        name: "fixture_test_module",
        purpose: "a module with no return-shape row",
        signatures: &["mcphost.fixture_test_module.ping() -- always returns true"],
    };
    let mut extended: Vec<BridgeModule> = BRIDGE_MODULES.to_vec();
    extended.push(fixture);
    let missing = bridge_functions_without_return_row(&extended, BRIDGE_RETURNS);
    assert_eq!(missing, vec!["mcphost.fixture_test_module.ping".to_string()]);
}
