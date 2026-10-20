//! PRD-mcphost-generated-code-map
//! AC2 — Given `src/db.rs` with its `// ---- <name> ----` banners, When the map
//! is generated, Then the db.rs entry lists every banner name in file order, and
//! a fixture file of 5,000+ lines with no banner appears under "large files
//! without sections".

use crate::codemap;

/// Banner names of `src/db.rs`, read independently of the generator.
fn db_banner_names() -> Vec<String> {
    let src = std::fs::read_to_string(codemap::repo().join("src/db.rs")).expect("src/db.rs");
    src.lines()
        .filter_map(|l| l.strip_prefix("// ---- "))
        .map(|rest| rest.trim_end().trim_end_matches('-').trim_end().to_string())
        .filter(|n| !n.is_empty())
        .collect()
}

#[test]
fn cmap_ac02_db_entry_lists_every_banner_in_file_order() {
    let root = codemap::real_src_copy("ac02-db");
    let out = codemap::run(&root, false);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let map = codemap::codemap(&root);

    let lines: Vec<&str> = map.lines().collect();
    let at = lines
        .iter()
        .position(|l| l.starts_with("- `src/db.rs` ("))
        .expect("db.rs entry");
    let sections = lines[at + 1]
        .trim_start()
        .strip_prefix("- sections: ")
        .expect("db.rs entry is followed by its sections line");
    let listed: Vec<&str> = sections.split(" | ").collect();
    let expected = db_banner_names();
    assert!(expected.len() >= 5, "db.rs should carry banners, got {expected:?}");
    assert_eq!(listed.len(), expected.len(), "{listed:?} vs {expected:?}");
    for (got, want) in listed.iter().zip(&expected) {
        // `:<digits>` is normalised away so the map carries no line-reference shape
        assert_eq!(got.replace(' ', ""), want.replace(' ', "").replace(':', ""), "banner order/name");
    }
}

#[test]
fn cmap_ac02_large_file_without_banner_is_listed_under_large_files() {
    let big: String = std::iter::once("//! A big fixture file with no section banners.\n".to_string())
        .chain((0..5_100).map(|i| format!("pub fn f{i}() {{}}\n")))
        .collect();
    let banner: String = std::iter::once("//! A big fixture file with banners.\n".to_string())
        .chain((0..5_100).map(|i| match i {
            0 => "// ---- only section ----\n".to_string(),
            _ => format!("pub fn g{i}() {{}}\n"),
        }))
        .collect();
    let root = codemap::fixture(
        "ac02-large",
        &[
            ("src/big.rs", big),
            ("src/sectioned.rs", banner),
            ("src/small.rs", codemap::src_file("Small.")),
        ],
    );
    let out = codemap::run(&root, false);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let map = codemap::codemap(&root);

    let large = codemap::section(&map, "Large files without sections");
    assert!(large.contains("`src/big.rs`"), "big.rs under the heading: {large}");
    assert!(!large.contains("sectioned.rs"), "a bannered file is not listed: {large}");
    assert!(!large.contains("small.rs"), "a small file is not listed: {large}");
    assert!(map.contains("  - sections: only section"), "banner listed: {map}");
}
