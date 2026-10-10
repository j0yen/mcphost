//! AC1 (PRD-mcphost-docs-external-links-resolve) -- Given a markdown file
//! with one 200 link, one 404 link, and one `cursor://` link, When
//! `docs-link-check.sh` runs on it, Then it exits 1, prints one
//! `FAIL 404 <url> <file>:<line>` line, and the `cursor://` link is
//! neither checked nor reported.

use crate::xlinks;

#[test]
fn a_404_fails_with_file_line_and_cursor_scheme_is_ignored() {
    let port = xlinks::serve();
    let dir = xlinks::scratch("ac01");
    let file = dir.join("doc.md");
    let ok = format!("http://127.0.0.1:{port}/ok");
    let gone = format!("http://127.0.0.1:{port}/gone");
    std::fs::write(
        &file,
        format!(
            "# t\n\n[fine]({ok})\n\n[dead]({gone})\n\n[Add](cursor://anysphere.cursor-deeplink/mcp/install?name=x)\n"
        ),
    )
    .unwrap();

    let out = xlinks::run_checker(&[&file]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "stdout: {stdout}");
    let fails: Vec<&str> = stdout.lines().filter(|l| l.starts_with("FAIL ")).collect();
    assert_eq!(fails, vec![format!("FAIL 404 {gone} {}:5", file.display())], "stdout: {stdout}");
    assert!(!stdout.contains("cursor"), "cursor:// must be neither checked nor reported: {stdout}");
    assert!(stdout.contains(&format!("ok 200 {ok}")), "the 200 link is checked: {stdout}");
}
