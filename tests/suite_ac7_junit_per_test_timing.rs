//! PRD-mcphost-test-suite-consolidation
//! AC7 (P1) -- Given `.config/nextest.toml` with JUnit enabled, When the
//! suite runs, Then a JUnit file exists with a duration per test.
//!
//! Asserted statically against the config's own content (not by actually
//! invoking `cargo nextest run` from inside a test, which would need
//! `cargo-nextest` on `$PATH` -- absent in CI's `gate`/`sandbox` jobs,
//! which run plain `cargo test`) -- nextest's JUnit writer always includes a
//! `time` attribute per `<testcase>` when the `[profile.default.junit]`
//! table is present with a `path`, which is what this checks for.

use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn nextest_toml_enables_junit_output() {
    let path = repo_root().join(".config/nextest.toml");
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));

    assert!(
        text.contains("[profile.default.junit]"),
        "{} must declare a [profile.default.junit] table",
        path.display()
    );
    assert!(
        text.lines().any(|l| {
            let l = l.trim();
            l.starts_with("path") && l.contains('=')
        }),
        "{} must set a `path` under [profile.default.junit] (nextest writes \
         one <testcase> per test with a `time` attribute to that file)",
        path.display()
    );
}
