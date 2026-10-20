//! PRD-mcphost-generated-code-map
//! AC4 — Given a fixture source file with no `//!` line in its first 5 lines,
//! When the generator runs, Then it exits non-zero, names that file, and leaves
//! `CODEMAP.md` unchanged; and the real tree passes.

use crate::codemap;

#[test]
fn cmap_ac04_missing_module_doc_exits_nonzero_names_file_writes_nothing() {
    // The `//!` sits on line 7: outside the first 5 lines, so it does not count.
    let late = "// a\n// b\n// c\n// d\n// e\n\n//! Too late to count.\npub fn f() {}\n";
    let root = codemap::fixture(
        "ac04",
        &[
            ("src/ok.rs", codemap::src_file("Fine.")),
            ("src/nodoc.rs", "pub fn f() {}\n".to_string()),
            ("src/late.rs", late.to_string()),
            ("CODEMAP.md", "previous map\n".to_string()),
        ],
    );
    let out = codemap::run(&root, false);
    assert!(!out.status.success(), "must exit non-zero");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("src/nodoc.rs"), "names the file: {err}");
    assert!(err.contains("src/late.rs"), "names the late-doc file: {err}");
    assert!(!err.contains("src/ok.rs"), "does not blame a good file: {err}");
    assert_eq!(codemap::codemap(&root), "previous map\n", "CODEMAP.md untouched");

    let check = codemap::run(&root, true);
    assert!(!check.status.success(), "--check also refuses");
}

#[test]
fn cmap_ac04_real_tree_passes() {
    let root = codemap::real_src_copy("ac04-real");
    let out = codemap::run(&root, false);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}
