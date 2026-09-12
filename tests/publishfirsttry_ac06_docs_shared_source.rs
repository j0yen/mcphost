//! AC6 (PRD-mcphost-publish-first-try requirement 6): "Given the README and
//! the served descriptions, When the docs test runs, Then both are rendered
//! from the same docs/kinds/*.md and match."
//!
//! `mcphost::kinds::docs::render_readme_section` renders `docs/kinds/*.md`
//! into the exact block README.md embeds between its `<!-- kinds:start -->`
//! / `<!-- kinds:end -->` markers; this test regenerates that block fresh
//! and diffs it against what's actually checked into README.md, so an edit
//! to one that isn't mirrored in the other fails here. The "served
//! description" side is covered by proving every registered kind's
//! `Kind::example()` -- what `host.tool_publish`'s wire description and
//! `host.quickstart` actually serve (`tests/publishfirsttry_ac01_*.rs`
//! already asserts the served description embeds each kind's example) --
//! is derived from the very same `docs/kinds/*.md` files via
//! `docs::parse_kind_doc`, not a hand-duplicated literal that could drift
//! from either the README or the file on disk.

use crate::common;

use mcphost::kinds::Kind;
use mcphost::kinds::docs::parse_kind_doc;
use mcphost::kinds::echo::EchoKind;
use mcphost::kinds::http::HttpKind;
use mcphost::kinds::python::PythonKind;

const README: &str = include_str!("../README.md");
const START_MARKER: &str = "<!-- kinds:start -->";
const END_MARKER: &str = "<!-- kinds:end -->";

#[test]
fn readme_kinds_section_matches_docs_kinds_render() {
    let start = README
        .find(START_MARKER)
        .expect("README.md must have a <!-- kinds:start --> marker")
        + START_MARKER.len();
    let end = README
        .find(END_MARKER)
        .expect("README.md must have a <!-- kinds:end --> marker");
    assert!(
        start <= end,
        "kinds:start marker must precede kinds:end marker"
    );
    let actual = README[start..end].trim();
    let expected = mcphost::kinds::docs::render_readme_section();
    assert_eq!(
        actual,
        expected.trim(),
        "README.md's Kinds section has drifted from docs/kinds/*.md -- \
         regenerate it from mcphost::kinds::docs::render_readme_section()"
    );
}

#[test]
fn echo_example_is_sourced_from_its_docs_file() {
    let doc = include_str!("../docs/kinds/echo.md");
    let from_doc = parse_kind_doc(doc);
    let from_kind = EchoKind.example();
    assert_eq!(from_kind.blurb, from_doc.blurb);
    assert_eq!(from_kind.spec, from_doc.spec);
    assert_eq!(from_kind.call_args, from_doc.call_args);
}

#[test]
fn http_example_is_sourced_from_its_docs_file() {
    let doc = include_str!("../docs/kinds/http.md");
    let from_doc = parse_kind_doc(doc);
    let lookup: std::sync::Arc<dyn mcphost::kinds::http::NameLookup> =
        std::sync::Arc::new(common::FixedLookup(std::collections::HashMap::new()));
    let from_kind = HttpKind::for_test("127.0.0.1", lookup).example();
    assert_eq!(from_kind.blurb, from_doc.blurb);
    assert_eq!(from_kind.spec, from_doc.spec);
    assert_eq!(from_kind.call_args, from_doc.call_args);
}

#[test]
fn python_example_is_sourced_from_its_docs_file() {
    let doc = include_str!("../docs/kinds/python.md");
    let from_doc = parse_kind_doc(doc);
    let data_dir = common::TempDataDir::new();
    let from_kind = PythonKind::new(&data_dir.0).example();
    assert_eq!(from_kind.blurb, from_doc.blurb);
    assert_eq!(from_kind.spec, from_doc.spec);
    assert_eq!(from_kind.call_args, from_doc.call_args);
}
