//! PRD-mcphost-transactions-only-via-helper AC2 -- lock parity. The frozen
//! `tests/txnhelp_parity_table.tsv` (the predecessor's 51 allow-listed sites,
//! keyed by (path, fn)) says which `TxBehavior` each function must call
//! `with_tx` with; the table's (path, fn) set must equal the set of `with_tx`
//! callers minus the predecessor's 8. One enclosing-fn scanner serves it
//! (`dryspt_lint`); no second copy may exist.

use crate::dryspt_lint;

use std::collections::{BTreeMap, BTreeSet};

/// The 8 functions the predecessor moved onto `with_tx` itself.
const PREDECESSOR_EIGHT: [(&str, &str); 8] = [
    ("src/db.rs", "state_table_drop"),
    ("src/db.rs", "documents_put"),
    ("src/db.rs", "document_soft_delete"),
    ("src/db.rs", "set_agent_profile"),
    ("src/db.rs", "purge_end_user"),
    ("src/db.rs", "revoke_all_oauth_grants_for_tenant"),
    ("src/index.rs", "index_create"),
    ("src/index.rs", "index_delete"),
];

fn root() -> &'static std::path::Path {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// (path, fn) -> sorted behaviours the table expects (one per old site).
fn table() -> (usize, BTreeMap<(String, String), Vec<String>>) {
    let text = std::fs::read_to_string(root().join("tests/txnhelp_parity_table.tsv")).expect("parity table");
    let mut rows = 0;
    let mut map: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let cols: Vec<&str> = line.split('\t').collect();
        assert_eq!(cols.len(), 4, "path, fn, matched token, behaviour: {line:?}");
        let expected = match cols[2] {
            "BEGIN IMMEDIATE" => "Immediate",
            "unchecked_transaction(" | "BEGIN;" => "Deferred",
            other => panic!("unknown token {other:?}"),
        };
        assert_eq!(cols[3], expected, "the behaviour column follows the old token: {line:?}");
        map.entry((cols[0].to_string(), cols[1].to_string())).or_default().push(cols[3].to_string());
        rows += 1;
    }
    for v in map.values_mut() {
        v.sort();
    }
    (rows, map)
}

fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).expect("read_dir") {
        let p = e.unwrap().path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn txnhelp_ac02() {
    let (rows, expected) = table();
    assert_eq!(rows, 51, "the frozen table has one row per predecessor-allow-listed site");

    let calls = dryspt_lint::scan_with_tx_src(root());
    assert!(calls.iter().all(|c| c.behavior != "?"), "every with_tx call names its behaviour: {calls:#?}");
    let mut actual: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for c in &calls {
        actual.entry((c.path.clone(), c.func.clone())).or_default().push(c.behavior.clone());
    }
    for v in actual.values_mut() {
        v.sort();
    }

    // Behaviour parity: each table function calls with_tx with its old mode.
    for (key, want) in &expected {
        assert_eq!(actual.get(key), Some(want), "{}::{} must call with_tx with {want:?}", key.0, key.1);
    }

    // Set equality: with_tx callers minus the predecessor's 8 == the table.
    let eight: BTreeSet<(String, String)> =
        PREDECESSOR_EIGHT.iter().map(|(p, f)| (p.to_string(), f.to_string())).collect();
    let callers: BTreeSet<(String, String)> = actual.keys().cloned().collect();
    assert!(eight.is_subset(&callers), "the predecessor's 8 still call with_tx: {:?}", eight.difference(&callers));
    let rest: BTreeSet<(String, String)> = callers.difference(&eight).cloned().collect();
    let table_keys: BTreeSet<(String, String)> = expected.keys().cloned().collect();
    assert_eq!(rest, table_keys, "with_tx callers minus the 8 equal the table's (path, fn) set");

    // One scanner: exactly one file defines the enclosing-fn scanner.
    let needle = format!("fn {}(", "enclosing_fns");
    let mut files = Vec::new();
    walk(&root().join("tests"), &mut files);
    walk(&root().join("src"), &mut files);
    let copies: Vec<_> = files
        .iter()
        .filter(|f| std::fs::read_to_string(f).is_ok_and(|t| t.contains(&needle)))
        .map(|f| f.strip_prefix(root()).unwrap().display().to_string())
        .collect();
    assert_eq!(copies, vec!["tests/support/dryspt_lint.rs".to_string()], "exactly one enclosing-fn scanner");
}
