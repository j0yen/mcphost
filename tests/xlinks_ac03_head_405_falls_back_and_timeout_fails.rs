//! AC3 (PRD-mcphost-docs-external-links-resolve) -- Given a host that
//! answers HEAD with 405 and GET with 200, When checked, Then it passes;
//! Given a host that never answers within the timeout, Then it fails with
//! status `timeout` after one retry.
//!
//! The script's timeout defaults to 10 s (asserted below against the
//! script text); the runner sets `DOCS_LINK_TIMEOUT=1` so the hang case
//! costs ~2 s (attempt + one retry) instead of ~20.

use crate::xlinks;

#[test]
fn head_405_then_get_200_passes() {
    let port = xlinks::serve();
    let dir = xlinks::scratch("ac03a");
    let file = dir.join("doc.md");
    let url = format!("http://127.0.0.1:{port}/head405");
    std::fs::write(&file, format!("[x]({url})\n")).unwrap();

    let out = xlinks::run_checker(&[&file]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "stdout: {stdout}");
    assert!(stdout.contains(&format!("ok 200 {url}")), "{stdout}");
}

#[test]
fn a_host_that_never_answers_fails_with_timeout() {
    let port = xlinks::serve();
    let dir = xlinks::scratch("ac03b");
    let file = dir.join("doc.md");
    let url = format!("http://127.0.0.1:{port}/hang");
    std::fs::write(&file, format!("[x]({url})\n")).unwrap();

    let started = std::time::Instant::now();
    let out = xlinks::run_checker(&[&file]);
    let elapsed = started.elapsed();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "stdout: {stdout}");
    assert!(stdout.contains(&format!("FAIL timeout {url} {}:1", file.display())), "{stdout}");
    // 1 s timeout x (first attempt + one retry): HEAD times out, so the
    // attempt ends there each time -- at least two timeouts elapsed.
    assert!(elapsed.as_millis() >= 1900, "expected a retry (>= 2 timeouts), took {elapsed:?}");
}

#[test]
fn default_timeout_is_ten_seconds() {
    let script = std::fs::read_to_string(xlinks::repo_root().join("scripts/docs-link-check.sh")).unwrap();
    assert!(script.contains(r#"os.environ.get("DOCS_LINK_TIMEOUT", "10")"#));
}
