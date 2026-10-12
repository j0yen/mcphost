//! AC3: the committed `examples/index.json` is byte-identical to a fresh
//! regeneration, and the embedded `EXAMPLES` has one entry per directory. A
//! copy of the tree with one extra directory and no manifest fails the check
//! naming that directory.

use std::path::{Path, PathBuf};

use mcphost::examples::{EXAMPLES, INDEX_FILE, collect, example_dirs, registered_kind_names, render_index};

fn real_examples() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("examples")
}

/// Directories, manifests and the committed index must agree.
fn drift(examples: &Path) -> Result<(), String> {
    let kinds = registered_kind_names();
    let entries = collect(examples, &kinds)
        .map_err(|errs| errs.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("\n"))?;
    let committed = std::fs::read_to_string(examples.join(INDEX_FILE)).map_err(|e| format!("{INDEX_FILE}: {e}"))?;
    if committed != render_index(&entries) {
        return Err(format!("{INDEX_FILE} differs from a fresh regeneration; run mcphost examples index"));
    }
    Ok(())
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().map(|e| e.unwrap()) {
        let dest = to.join(e.file_name());
        if e.path().is_dir() {
            copy_dir(&e.path(), &dest);
        } else {
            std::fs::copy(e.path(), dest).unwrap();
        }
    }
}

#[test]
fn committed_index_is_byte_identical_to_regeneration() {
    if let Err(why) = drift(&real_examples()) {
        panic!("{why}");
    }
}

#[test]
fn embedded_examples_match_the_directories() {
    let dirs = example_dirs(&real_examples()).unwrap();
    assert_eq!(EXAMPLES.len(), dirs.len());
    let names: Vec<&str> = EXAMPLES.iter().map(|e| e.name).collect();
    let dir_names: Vec<String> = dirs.iter().map(|d| d.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names, dir_names, "EXAMPLES must be sorted by name and cover every directory");
    for ex in EXAMPLES {
        for t in ex.tools {
            let on_disk = std::fs::read_to_string(real_examples().join(ex.name).join(t.file)).unwrap();
            assert_eq!(t.source, on_disk, "{}/{} embedded source differs from disk", ex.name, t.file);
        }
    }
}

#[test]
fn an_extra_directory_without_a_manifest_fails_naming_it() {
    let tmp = std::env::temp_dir().join(format!("exmf-ac03-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    copy_dir(&real_examples(), &tmp);
    assert_eq!(drift(&tmp), Ok(()), "the copy must start green");
    std::fs::create_dir(tmp.join("stray-example")).unwrap();
    let why = drift(&tmp).unwrap_err();
    assert!(why.contains("stray-example"), "{why}");
    let _ = std::fs::remove_dir_all(&tmp);
}
