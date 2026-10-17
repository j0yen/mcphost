//! PRD-mcphost-skill-md-generated-from-plugin
//! AC4 (P0) -- Given a tree where every generated file is current, When
//! `mcphost gen-docs` runs twice, Then the second run changes no file.

use crate::common;
use std::collections::BTreeMap;
use std::path::Path;

fn snapshot(dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            snapshot(&p, out);
        } else {
            out.insert(p.display().to_string(), std::fs::read(&p).unwrap());
        }
    }
}

#[test]
fn skillsrc_ac04() {
    let dir = common::gen_docs_scratch_tree("skillsrc-ac04");
    let first = common::run_gen_docs(&dir, &[]);
    assert!(first.status.success(), "{}", String::from_utf8_lossy(&first.stderr));
    let mut before = BTreeMap::new();
    snapshot(&dir, &mut before);
    let second = common::run_gen_docs(&dir, &[]);
    assert!(second.status.success(), "{}", String::from_utf8_lossy(&second.stderr));
    let mut after = BTreeMap::new();
    snapshot(&dir, &mut after);
    assert_eq!(before, after, "the second gen-docs run changed a file");
}
