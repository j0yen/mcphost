//! PRD-mcphost-invalid-spec-message-carries-fix
//! AC5 (P0) — Given the error registry, When the spmsg drift test runs over every
//! entry, Then every `data.<key>` token in an entry's `remedy` is in that entry's
//! `data_keys` (the `invalid_spec` remedy no longer names `data.expected`), and
//! `docs/errors.md` equals its generator's output with no hand edit.
//!
//! An entry that declares no `data_keys` has no registered key list to drift
//! from (its `data` is whatever its raising site builds), so the check binds the
//! entries that declare one -- the same population `docs/errors.md`'s `## Error
//! data keys` section is generated from.

use mcphost::error_registry::{ERROR_CODES, lookup};

/// Every `data.<key>` token of `text`: `data.` not glued to a preceding
/// identifier character, followed by an identifier.
fn data_tokens(text: &str) -> Vec<String> {
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    text.match_indices("data.")
        .filter(|(i, _)| !text[..*i].chars().next_back().is_some_and(is_ident))
        .map(|(i, m)| text[i + m.len()..].chars().take_while(|c| is_ident(*c)).collect::<String>())
        .filter(|k| !k.is_empty())
        .collect()
}

#[test]
fn spmsg_ac05() {
    let mut checked = 0;
    let mut failures = Vec::new();
    for entry in ERROR_CODES.iter().filter(|e| !e.data_keys.is_empty()) {
        checked += 1;
        for key in data_tokens(entry.remedy) {
            if !entry.data_keys.contains(&key.as_str()) {
                failures.push(format!("{}: remedy names data.{key}, not in data_keys {:?}", entry.code, entry.data_keys));
            }
        }
    }
    assert!(checked > 0, "the registry declares data_keys somewhere");
    assert!(failures.is_empty(), "{}", failures.join("\n"));

    let row = lookup("invalid_spec").expect("invalid_spec is a registry row");
    assert!(!row.remedy.contains("data.expected"), "{}", row.remedy);
    assert!(row.remedy.len() <= 200, "{} chars", row.remedy.len());
    let names = data_tokens(row.remedy);
    assert!(!names.is_empty() && names.iter().all(|k| row.data_keys.contains(&k.as_str())), "{names:?}");

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let errors_md = std::fs::read_to_string(root.join("docs/errors.md")).expect("read docs/errors.md");
    assert_eq!(errors_md, mcphost::retry_policy::render_doc(), "docs/errors.md is not its generator's output");
}
