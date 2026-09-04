//! Single source for every registered kind's "what does a minimal example
//! spec look like" content, embedded from `docs/kinds/*.md` at compile time
//! (PRD-mcphost-publish-first-try requirement 6 / AC6). [`parse_kind_doc`]
//! turns one such file into a [`KindExample`] -- every kind's
//! `Kind::example()` (`echo.rs`, `http.rs`, `python.rs`) calls it on its own
//! file rather than hand-duplicating the blurb/spec/call_args literals that
//! used to live directly in `src/kinds/*.rs`, so `host.tool_publish`'s wire
//! description and `host.quickstart` can never say something the crate's
//! own README doesn't. [`render_readme_section`] renders the same three
//! files into the exact block README.md embeds between its
//! `<!-- kinds:start -->` / `<!-- kinds:end -->` markers;
//! `tests/publishfirsttry_ac06_docs_shared_source.rs` regenerates that
//! block and diffs it against what's actually checked into README.md, so a
//! `docs/kinds/*.md` edit that isn't mirrored in the README fails CI.

use serde_json::Value;

use super::KindExample;

/// Parses one `docs/kinds/<name>.md` file's content into a [`KindExample`]:
/// the first non-empty, non-fence line is the blurb sentence; the first
/// fenced ```json block is the example spec; the second fenced ```json
/// block is the example call arguments. A missing block falls back to
/// `Value::Null` (spec) / `{}` (call_args) rather than panicking, so a
/// malformed doc degrades to an empty-but-valid example instead of taking
/// the whole binary down.
pub fn parse_kind_doc(markdown: &str) -> KindExample {
    let blurb = markdown
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#') && !line.starts_with("```"))
        .unwrap_or("")
        .to_string();

    let mut blocks: Vec<Value> = Vec::new();
    let mut in_block = false;
    let mut buf = String::new();
    for line in markdown.lines() {
        let trimmed = line.trim();
        if !in_block && trimmed.starts_with("```json") {
            in_block = true;
            buf.clear();
        } else if in_block && trimmed.starts_with("```") {
            in_block = false;
            if let Ok(value) = serde_json::from_str(&buf) {
                blocks.push(value);
            }
        } else if in_block {
            buf.push_str(line);
            buf.push('\n');
        }
    }

    let mut iter = blocks.into_iter();
    let spec = iter.next().unwrap_or(Value::Null);
    let call_args = iter.next().unwrap_or_else(|| serde_json::json!({}));
    KindExample {
        spec,
        call_args,
        blurb,
    }
}

/// One entry per registered kind, in the order `host.tool_publish`'s
/// description lists them. Kept here (not derived from a live
/// `KindRegistry`) because the description string is built in `handler.rs`
/// from whichever kinds `main.rs` actually registered -- this constant only
/// needs to cover every doc file that exists, for the README render.
const KIND_DOCS: &[(&str, &str)] = &[
    ("echo", include_str!("../../docs/kinds/echo.md")),
    ("http", include_str!("../../docs/kinds/http.md")),
    ("python", include_str!("../../docs/kinds/python.md")),
];

fn render_kind_section(name: &str, markdown: &str) -> String {
    let example = parse_kind_doc(markdown);
    let spec_json = serde_json::to_string_pretty(&example.spec).unwrap_or_default();
    let call_args_json = serde_json::to_string_pretty(&example.call_args).unwrap_or_default();
    format!(
        "### `{name}`\n\n{blurb}\n\nExample spec:\n\n```json\n{spec_json}\n```\n\nExample call arguments:\n\n```json\n{call_args_json}\n```",
        blurb = example.blurb,
    )
}

/// The full "Kinds" section README.md embeds between its
/// `<!-- kinds:start -->` / `<!-- kinds:end -->` markers -- every kind in
/// [`KIND_DOCS`], rendered by [`render_kind_section`] and joined with a
/// blank line between sections.
pub fn render_readme_section() -> String {
    KIND_DOCS
        .iter()
        .map(|(name, markdown)| render_kind_section(name, markdown))
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_blurb_and_both_json_blocks() {
        let example = parse_kind_doc(
            "the blurb sentence.\n\n```json\n{\"a\": 1}\n```\n\nCall arguments:\n\n```json\n{\"b\": 2}\n```\n",
        );
        assert_eq!(example.blurb, "the blurb sentence.");
        assert_eq!(example.spec, serde_json::json!({"a": 1}));
        assert_eq!(example.call_args, serde_json::json!({"b": 2}));
    }

    #[test]
    fn every_kind_doc_file_parses_to_a_non_empty_blurb_and_spec() {
        for (name, markdown) in KIND_DOCS {
            let example = parse_kind_doc(markdown);
            assert!(!example.blurb.is_empty(), "{name}: blurb must not be empty");
            assert!(
                !example.spec.is_null(),
                "{name}: spec must parse to real JSON"
            );
        }
    }
}
