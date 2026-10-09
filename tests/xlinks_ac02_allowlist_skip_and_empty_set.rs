//! AC2 (PRD-mcphost-docs-external-links-resolve) -- Given a file whose only
//! external URL matches an allowlist pattern (`api.example.com`), When the
//! checker runs, Then it prints `SKIP` with the reason and exits 0; Given
//! an empty file list, Then it exits 0 with `0 urls`.

use crate::xlinks;

#[test]
fn allowlisted_url_is_skipped_with_reason_and_exit_zero() {
    let dir = xlinks::scratch("ac02a");
    let file = dir.join("doc.md");
    std::fs::write(&file, "call https://api.example.com/v1/things for a thing\n").unwrap();

    let out = xlinks::run_checker(&[&file]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "stdout: {stdout}");
    let skip = stdout.lines().find(|l| l.starts_with("SKIP ")).unwrap_or_else(|| panic!("no SKIP line: {stdout}"));
    assert!(skip.contains("https://api.example.com/v1/things"), "{skip}");
    assert!(skip.contains("documentation placeholder host"), "SKIP carries the allowlist reason: {skip}");
    assert!(!stdout.contains("FAIL"), "{stdout}");
}

#[test]
fn empty_file_list_is_zero_urls_and_exit_zero() {
    let dir = xlinks::scratch("ac02b");
    let file = dir.join("empty.md");
    std::fs::write(&file, "").unwrap();

    let out = xlinks::run_checker(&[&file]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "stdout: {stdout}");
    assert!(stdout.contains("0 urls"), "{stdout}");
}
