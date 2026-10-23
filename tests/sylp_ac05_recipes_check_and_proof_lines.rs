//! contract: reads recipes/*.toml, examples/data-sync/proof.sh
//! sylp AC5 (PRD-mcphost-sync-pull-change-detection-live-proof R5): after the
//! recipe edit `cargo xtask recipes --check` exits 0, and the committed
//! `examples/data-sync/proof.sh` prints two `host.sync.get:` lines, each
//! carrying `last_run.status=`, `rows_added=`, `change_detected=` and `waited_s=`.

use std::path::Path;

fn repo() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn sylp_ac05_recipes_check_exits_zero() {
    // The same entry point `cargo xtask recipes --check` calls.
    let code = mcphost::recipes::cli(repo(), &["--check".to_string()]);
    assert_eq!(code, 0, "cargo xtask recipes --check must exit 0 (run `cargo xtask recipes`)");
    assert!(mcphost::recipes::drift(repo()).expect("recipes drift").is_empty());
}

#[test]
fn sylp_ac05_committed_proof_prints_two_get_lines_with_the_receipt_fields() {
    let proof = std::fs::read_to_string(repo().join("examples/data-sync/proof.sh")).expect("read proof.sh");
    let lines: Vec<&str> = proof.lines().filter(|l| l.trim_start().starts_with("echo \"host.sync.get: ")).collect();
    assert_eq!(lines.len(), 2, "two host.sync.get: echo lines (first and second get): {lines:?}");
    for l in lines {
        for field in ["last_run.status=", "rows_added=", "change_detected=", "waited_s="] {
            assert!(l.contains(field), "get line lacks {field}: {l}");
        }
    }
    // The polled loop is shared, so the two get lines run it once per run_now.
    assert_eq!(proof.matches("host.sync.run_now\" ").count(), 2, "two run_now calls");
    assert_eq!(proof.matches("poll_sync_get \"(").count(), 2, "two polled gets");
}
