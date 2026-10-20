//! PRD-mcphost-generated-code-map
//! AC3 — Given a fixture tree with a top-level `pub const FOO: &[&str]` registry,
//! When the map is generated, Then the Registries section lists `FOO` with its
//! file.

use crate::codemap;

#[test]
fn cmap_ac03_registries_lists_top_level_array_registries_with_file() {
    let src = "//! Fixture with registries.\n\
               pub const FOO: &[&str] = &[\"a\", \"b\"];\n\
               pub static BAR: &'static [u8] = &[1];\n\
               pub const SCALAR: u32 = 7;\n\
               pub const NAMES: [&str; 1] = [\"x\"];\n\
               fn inner() {\n    pub const NESTED: &[&str] = &[];\n}\n";
    let root = codemap::fixture(
        "ac03",
        &[("src/reg.rs", src.to_string()), ("src/other.rs", codemap::src_file("Other."))],
    );
    let out = codemap::run(&root, false);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let reg = codemap::section(&codemap::codemap(&root), "Registries");

    assert!(reg.contains("- `FOO` in `src/reg.rs`"), "FOO with its file: {reg}");
    assert!(reg.contains("- `BAR` in `src/reg.rs`"), "static slice registry: {reg}");
    for not in ["SCALAR", "NAMES", "NESTED"] {
        assert!(!reg.contains(not), "{not} is not a top-level `&[` registry: {reg}");
    }
}
