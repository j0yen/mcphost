//! AC2 (PRD-mcphost-chatgpt-submission-pack) — Given a committed
//! `submission/chatgpt/` equal to the generator output, When `--check` runs,
//! Then exit 0; after the shared description changes in source, exit
//! non-zero naming `metadata.json`.

use mcphost::submission_pack;
use std::process::Command;

fn check(dir: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["submission-pack", "--check", "--chatgpt"])
        .arg(dir)
        .env_remove("MCPHOST_PUBLIC_URL")
        .output()
        .expect("run mcphost submission-pack --check")
}

#[test]
fn committed_pack_passes_check_and_description_drift_names_metadata_json() {
    let committed = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(submission_pack::PACK_PATH);
    let ok = check(&committed);
    assert!(ok.status.success(), "committed pack must be current: {}", String::from_utf8_lossy(&ok.stderr));

    // The shared description changes in source: the committed copy is now stale.
    let changed = submission_pack::render_pack("https://mcphost.dev", "A different shared description.");
    let drifted = submission_pack::check_pack(&committed, &changed);
    assert!(drifted.contains(&"metadata.json"), "{drifted:?}");
    assert!(!drifted.contains(&"icon-64.png"), "icon does not depend on the description: {drifted:?}");

    // Same through the binary: a committed copy that disagrees with the
    // generator exits non-zero and names the file.
    let stale = std::env::temp_dir().join(format!("mcphost-pack-ac02-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&stale);
    submission_pack::write_pack(&stale, &changed).expect("write stale pack");
    let bad = check(&stale);
    assert!(!bad.status.success(), "stale pack must fail --check");
    assert!(String::from_utf8_lossy(&bad.stderr).contains("metadata.json"), "{}", String::from_utf8_lossy(&bad.stderr));
    let _ = std::fs::remove_dir_all(&stale);
}
