//! PRD-mcphost-host-doctor-next-names-envelope-key
//! AC3 -- Given every `BRIDGE_RETURNS` envelope row, When the runner's
//! wrapper (`_wrap_envelope`, the one every installed bridge function goes
//! through) wraps the row's rendered example, Then the wrapper's keys equal
//! the shape's top-level keys; a planted extra key in a copy of the table
//! fails naming the fqn.

use mcphost::kinds::python::{BRIDGE_RETURNS, ReturnShape, runner_script_module_prelude};
use std::collections::BTreeMap;
use std::io::Write;
use std::process::{Command, Stdio};

/// `{fqn: wrapped envelope keys}` for every envelope row, from the runner:
/// the example is built from the row's own rendered `returns` text in the
/// runner's `_BRIDGE_RETURNS`, then pushed through `_wrap_envelope`.
fn runner_wrapped_keys() -> BTreeMap<String, Vec<String>> {
    let mut script = runner_script_module_prelude().to_string();
    script.push_str(
        r#"
import re
out = {}
for fqn, info in _BRIDGE_RETURNS.items():
    if not info["envelope"]:
        continue
    keys = re.findall(r'"([^"]+)": ', info["returns"])
    example = {k: 1 for k in keys}
    base = fqn.partition("(")[0]
    wrapper = _wrap_envelope(lambda *a, **kw: dict(example), base, _shape_message(fqn, info), [])
    env = wrapper()
    assert isinstance(env, _Envelope), fqn
    out[fqn] = sorted(dict.keys(env))
    if "(" not in fqn:
        mod, name = _bridge_target(fqn)
        assert hasattr(getattr(mod, name), "__wrapped__"), fqn + " is not installed through _wrap_envelope"
print("KEYS " + json.dumps(out))
"#,
    );
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
    let line = stdout.lines().find_map(|l| l.strip_prefix("KEYS ")).expect("KEYS line");
    serde_json::from_str(line).unwrap()
}

/// The fqns whose wrapper keys differ from the table's, as messages.
fn drift(table: &BTreeMap<String, Vec<String>>, wrapped: &BTreeMap<String, Vec<String>>) -> Vec<String> {
    let mut bad = Vec::new();
    for (fqn, want) in table {
        match wrapped.get(fqn) {
            Some(got) if got == want => {}
            got => bad.push(format!("{fqn}: table keys {want:?} != wrapper keys {got:?}")),
        }
    }
    bad
}

fn table_keys() -> BTreeMap<String, Vec<String>> {
    BRIDGE_RETURNS
        .iter()
        .filter_map(|r| match r.shape {
            ReturnShape::Envelope(keys) => {
                let mut k: Vec<String> = keys.iter().map(|(k, _)| k.to_string()).collect();
                k.sort();
                Some((r.function.to_string(), k))
            }
            ReturnShape::Plain(_) => None,
        })
        .collect()
}

#[test]
fn hdnext_ac03() {
    let wrapped = runner_wrapped_keys();
    let table = table_keys();
    assert!(table.contains_key("mcphost.state.query"), "{table:?}");
    let bad = drift(&table, &wrapped);
    assert!(bad.is_empty(), "wrapper keys drifted from BRIDGE_RETURNS: {bad:#?}");

    let mut planted = table.clone();
    planted.get_mut("mcphost.state.query").unwrap().push("planted_extra".to_string());
    let bad = drift(&planted, &wrapped);
    assert_eq!(bad.len(), 1, "{bad:?}");
    assert!(bad[0].starts_with("mcphost.state.query:") && bad[0].contains("planted_extra"), "{bad:?}");
}
