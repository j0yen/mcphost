//! PRD-mcphost-generated-code-map
//! AC8 — Given a fixture copy where only one function body in a 2,000+ line file
//! changes (same line-count class, no banner, `//!` or registry change), When the
//! map is regenerated, Then `CODEMAP.md` is byte-identical.

use crate::codemap;

fn big_file(body: &str) -> String {
    let mut s = String::from("//! A large fixture module.\n\n// ---- part one ----\n");
    s.push_str("pub const REG: &[&str] = &[\"a\"];\n");
    for i in 0..2_100 {
        if i == 1_000 {
            s.push_str(&format!("pub fn target() -> u32 {{ {body} }}\n"));
        } else {
            s.push_str(&format!("pub fn f{i}() -> u32 {{ {i} }}\n"));
        }
    }
    s
}

#[test]
fn cmap_ac08_function_body_edit_leaves_codemap_byte_identical() {
    let root = codemap::fixture(
        "ac08",
        &[("src/big.rs", big_file("1")), ("src/small.rs", codemap::src_file("Small."))],
    );
    assert!(codemap::run(&root, false).status.success());
    let before = std::fs::read(root.join("CODEMAP.md")).expect("map");

    codemap::write(&root, "src/big.rs", &big_file("2 + 3 * 4"));
    assert!(codemap::run(&root, false).status.success());
    let after = std::fs::read(root.join("CODEMAP.md")).expect("map");
    assert_eq!(before, after, "a body-only edit must not change CODEMAP.md");
    assert!(String::from_utf8_lossy(&before).contains("(~2k)"), "fixture is in the 2k class");
}
