//! PRD-mcphost-sandbox-file-processing-libs
//! AC2 (P0) -- Given `sandbox-libs.toml` and `sandbox-libs.lock`, When the
//! drift test runs, Then every toml `(name, version)` is in the lock, every
//! lock entry has a hash, and the rendered table in `docs/kinds/python.md`
//! and `www/llms.txt` equals `render_sandbox_libs` byte for byte; editing a
//! version in the toml alone makes it fail.

use mcphost::gendocs::render_sandbox_libs;
use mcphost::sandbox_libs::{check_drift, libs, parse_lock};

fn read(path: &str) -> String {
    std::fs::read_to_string(format!("{}/{path}", env!("CARGO_MANIFEST_DIR"))).expect(path)
}

#[test]
fn sblibs_ac02_committed_files_agree() {
    let toml = read("sandbox-libs.toml");
    let lock = read("sandbox-libs.lock");
    let python_md = read("docs/kinds/python.md");
    let llms = read("www/llms.txt");
    check_drift(&toml, &lock, &python_md, &llms).expect("toml, lock and docs must agree");

    assert_eq!(libs().len(), 5, "five preinstalled libs");
    let entries = parse_lock(&lock);
    for l in libs() {
        assert!(
            entries.iter().any(|(n, v, h)| n.eq_ignore_ascii_case(&l.name) && *v == l.version && !h.is_empty()),
            "{} {} must be in the lock with hashes",
            l.name,
            l.version
        );
    }
    assert!(entries.iter().all(|(_, _, h)| !h.is_empty()), "every lock entry has a hash");

    // Byte for byte, in both documents.
    let region = render_sandbox_libs(&toml);
    assert!(python_md.contains(&region), "docs/kinds/python.md carries the rendered table");
    assert!(llms.contains(&region), "www/llms.txt carries the rendered table");
}

#[test]
fn sblibs_ac02_editing_a_version_in_the_toml_alone_fails() {
    let toml = read("sandbox-libs.toml");
    let lock = read("sandbox-libs.lock");
    let python_md = read("docs/kinds/python.md");
    let llms = read("www/llms.txt");
    let edited = toml.replace("version = \"6.21.0\"", "version = \"6.21.1\"");
    assert_ne!(edited, toml, "the fixture edit must change the toml");
    let err = check_drift(&edited, &lock, &python_md, &llms).expect_err("a toml-only edit must fail");
    assert!(err.contains("pypdf"), "failure names the lib: {err}");
}

#[test]
fn sblibs_ac02_a_hand_edited_docs_table_fails() {
    let toml = read("sandbox-libs.toml");
    let lock = read("sandbox-libs.lock");
    let python_md = read("docs/kinds/python.md").replace("| pypdf | 6.21.0 |", "| pypdf | 6.20.0 |");
    let llms = read("www/llms.txt");
    assert!(check_drift(&toml, &lock, &python_md, &llms).is_err());
}
