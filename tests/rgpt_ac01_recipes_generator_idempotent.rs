//! PRD-mcphost-recipe-golden-path-tests
//! AC1 -- Given `recipes/*.toml`, When `cargo xtask recipes` runs twice, Then
//! the first run writes the sections, proof scripts, task yamls and golden
//! tests for every recipe and the second run changes zero bytes.
//!
//! The run happens in a scratch copy that holds only the toml inputs and a
//! `www/llms.txt` with every recipe block removed, so the first run has all
//! of its outputs to write; the committed tree must then equal what it wrote.

use crate::common;

use common::TempDataDir;
use mcphost::recipes;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn repo() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, String> {
    fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<PathBuf, String>) {
        for entry in std::fs::read_dir(dir).expect("read_dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                walk(&path, root, out);
            } else {
                let rel = path.strip_prefix(root).expect("under root").to_path_buf();
                out.insert(rel, std::fs::read_to_string(&path).expect("utf8"));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

fn scratch_with_inputs_only(scratch: &Path) -> Vec<recipes::Recipe> {
    let all = recipes::load_all(repo()).expect("load recipes");
    std::fs::create_dir_all(scratch.join("recipes")).expect("mkdir recipes");
    std::fs::create_dir_all(scratch.join("www")).expect("mkdir www");
    for r in &all {
        let name = &r.name;
        std::fs::copy(repo().join(format!("recipes/{name}.toml")), scratch.join(format!("recipes/{name}.toml")))
            .expect("copy toml");
    }
    let mut llms = std::fs::read_to_string(repo().join("www/llms.txt")).expect("read llms.txt");
    for r in &all {
        llms = recipes::strip_llms_block(&llms, &r.name);
        assert!(!llms.contains(&recipes::begin_marker(&r.name)), "{} block not stripped", r.name);
    }
    std::fs::write(scratch.join("www/llms.txt"), llms).expect("write llms.txt");
    all
}

#[test]
fn rgpt_ac01_first_run_writes_every_artifact_second_run_changes_zero_bytes() {
    let scratch = TempDataDir::new();
    let all = scratch_with_inputs_only(&scratch.0);
    assert!(all.len() >= 7, "expected a toml per recipe, found {}", all.len());

    let first = recipes::plan(&scratch.0).and_then(|files| recipes::write(&scratch.0, &files)).expect("first run");
    let after_first = snapshot(&scratch.0);
    for r in &all {
        let n = &r.name;
        for rel in [
            format!("examples/{n}/proof.sh"),
            format!("examples/{n}/synthorg-task.yaml"),
            format!("tests/rgpt_{}_golden_path.rs", n.replace('-', "_")),
        ] {
            assert!(first.iter().any(|p| p == Path::new(&rel)), "first run did not write {rel}: {first:?}");
            assert!(after_first.contains_key(Path::new(&rel)), "{rel} missing after the first run");
        }
        let llms = &after_first[Path::new("www/llms.txt")];
        assert!(llms.contains(&recipes::begin_marker(n)) && llms.contains(&recipes::end_marker(n)), "{n}: no llms.txt block");
    }

    let second = recipes::plan(&scratch.0).and_then(|files| recipes::write(&scratch.0, &files)).expect("second run");
    assert!(second.is_empty(), "second run changed files: {second:?}");
    assert_eq!(snapshot(&scratch.0), after_first, "second run changed bytes");

    // The committed tree is exactly what the generator writes.
    for (rel, content) in &after_first {
        if rel.starts_with("recipes") {
            continue;
        }
        let committed = std::fs::read_to_string(repo().join(rel)).unwrap_or_else(|e| panic!("{}: {e}", rel.display()));
        assert_eq!(&committed, content, "{} is not what `cargo xtask recipes` writes", rel.display());
    }
}
