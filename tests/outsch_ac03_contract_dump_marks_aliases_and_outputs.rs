//! PRD-mcphost-contract-output-schemas
//! AC3 (P0) -- Given `mcphost contract dump`, When the file is parsed, Then
//! every entry has exactly one of `output_schema` or `alias_of`, every
//! `alias_of` names an entry that has `output_schema`, and
//! `tool_aliases::aliases_of` agrees with the file for every canonical.

use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

use serde_json::Value;

fn dump_file() -> Value {
    // One directory per call: the two tests in this file run in parallel.
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("outsch-ac03-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("host-tools.v1.json");
    let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["contract", "dump", "--path"])
        .arg(&path)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run mcphost contract dump");
    assert!(out.status.success(), "contract dump failed: {}", String::from_utf8_lossy(&out.stderr));
    let parsed = serde_json::from_slice(&std::fs::read(&path).expect("dump written")).expect("dump is JSON");
    let _ = std::fs::remove_dir_all(&dir);
    parsed
}

#[test]
fn every_entry_has_exactly_one_of_output_schema_or_alias_of() {
    let dump = dump_file();
    let tools = dump["tools"].as_array().expect("tools");
    let by_name: BTreeMap<&str, &Value> = tools.iter().map(|t| (t["name"].as_str().expect("name"), t)).collect();
    let mut problems = Vec::new();
    let (mut canonicals, mut aliases) = (0, 0);
    for (name, entry) in &by_name {
        let has_schema = entry.get("output_schema").is_some();
        let alias_of = entry.get("alias_of");
        match (has_schema, alias_of) {
            (true, None) => canonicals += 1,
            (false, Some(target)) => {
                aliases += 1;
                let target = target.as_str().unwrap_or_default();
                if by_name.get(target).is_none_or(|t| t.get("output_schema").is_none()) {
                    problems.push(format!("{name}: alias_of {target} names no entry with an output_schema"));
                }
            }
            (true, Some(_)) => problems.push(format!("{name}: has both output_schema and alias_of")),
            (false, None) => problems.push(format!("{name}: has neither output_schema nor alias_of")),
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
    assert!(canonicals > 0 && aliases > 0, "canonicals {canonicals}, aliases {aliases}");
}

#[test]
fn aliases_of_agrees_with_the_file_for_every_canonical() {
    let dump = dump_file();
    let tools = dump["tools"].as_array().expect("tools");
    let mut claimed: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut canonicals = Vec::new();
    for t in tools {
        let name = t["name"].as_str().expect("name");
        match t.get("alias_of").and_then(Value::as_str) {
            Some(canonical) => {
                claimed.entry(canonical).or_default().insert(name);
            }
            None => canonicals.push(name),
        }
    }
    for canonical in canonicals {
        let in_file = claimed.get(canonical).cloned().unwrap_or_default();
        // The registry's deprecated aliases plus the flattened `a_b_c` form.
        let mut expected: BTreeSet<String> =
            mcphost::tool_aliases::aliases_of(canonical).into_iter().map(str::to_string).collect();
        expected.extend(mcphost::tool_aliases::flattened_form(canonical));
        let in_file: BTreeSet<String> = in_file.into_iter().map(str::to_string).collect();
        assert_eq!(in_file, expected, "{canonical}: alias_of entries disagree with tool_aliases");
    }
}
