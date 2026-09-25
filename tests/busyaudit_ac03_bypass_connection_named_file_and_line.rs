//! PRD-mcphost-sqlite-busy-timeout-audit
//! AC3 — Given a connection path that bypasses the factory (test injects
//! one), When the audit test runs, Then it fails naming the file and line.

use mcphost::db::find_bypass_connections;
use std::path::Path;

#[test]
fn real_src_tree_has_no_bypass_connections() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let hits = find_bypass_connections(&src);
    assert!(
        hits.is_empty(),
        "found `Connection::open(` outside db.rs (requirement 1 bypass): {hits:?}"
    );
}

#[test]
fn injected_bypass_is_named_with_its_file_and_line() {
    let scratch = std::env::temp_dir().join(format!(
        "mcphost-busyaudit-ac03-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(scratch.join("fake")).expect("create scratch tree");
    let injected_path = scratch.join("fake").join("bogus.rs");
    std::fs::write(
        &injected_path,
        "fn f() {\n    let c = Connection::open(path).unwrap();\n    drop(c);\n}\n",
    )
    .expect("write injected fixture");

    let hits = find_bypass_connections(&scratch);

    assert_eq!(hits.len(), 1, "expected exactly one bypass hit: {hits:?}");
    let hit = &hits[0];
    assert_eq!(hit.file, injected_path, "{hit:?}");
    assert_eq!(hit.line, 2, "{hit:?}");
    assert!(hit.text.contains("Connection::open("), "{hit:?}");

    let _ = std::fs::remove_dir_all(&scratch);
}
