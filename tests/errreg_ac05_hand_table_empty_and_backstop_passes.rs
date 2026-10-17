//! PRD-mcphost-error-codes-generated-registry
//! AC5 (P0) — Given the migrated tree, When the lib gate runs under
//! `cargo test --lib`, Then it passes, `src/error_registry.rs` contains zero
//! `row(` calls, and `tests/errjson_ac01_every_emitted_code_is_a_registry_row.rs`
//! passes unchanged (it runs in this same suite binary; this file proves the
//! other two halves and that the backstop file still scans the same shapes).

use mcphost::error_registry::{ERROR_CODES, REGISTERED};
use mcphost::error_scan::scan_tree;
use std::path::Path;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn errreg_ac05_hand_table_has_zero_row_calls() {
    let text = std::fs::read_to_string(root().join("src/error_registry.rs")).expect("read");
    let calls: Vec<(usize, &str)> = text
        .lines()
        .enumerate()
        .filter(|(_, l)| {
            let t = l.trim_start();
            !t.starts_with("//") && (t.starts_with("row(") || t.contains(" row(\"") || t.contains("[row("))
        })
        .collect();
    assert!(calls.is_empty(), "hand-table row( calls remain in src/error_registry.rs: {calls:?}");
}

#[test]
fn errreg_ac05_every_registry_entry_comes_from_a_declaration() {
    assert_eq!(ERROR_CODES.len(), REGISTERED.len(), "ERROR_CODES holds rows no err_code! declared");
    assert!(REGISTERED.len() > 250, "only {} declared codes", REGISTERED.len());
    for e in REGISTERED.iter() {
        assert!(e.site.starts_with("src/") && !e.site.starts_with("src/error_registry.rs"), "{} declared at {}", e.code, e.site);
    }
}

#[test]
fn errreg_ac05_scanner_passes_over_the_real_src_tree() {
    let found = scan_tree(&root().join("src")).expect("scan src");
    let lines: Vec<String> = found.iter().map(ToString::to_string).collect();
    assert!(found.is_empty(), "{}", lines.join("\n"));
}

#[test]
fn errreg_ac05_backstop_test_file_still_scans_the_four_prefixes_and_arms() {
    let text = std::fs::read_to_string(root().join("tests/errjson_ac01_every_emitted_code_is_a_registry_row.rs")).expect("read");
    for needle in ["\"code:\"", "\"structured(\"", "\"structured_with(\"", "fn code_arms", "errjson_ac01_every_emitted_code_is_a_registry_row"] {
        assert!(text.contains(needle), "backstop lost {needle}");
    }
}
