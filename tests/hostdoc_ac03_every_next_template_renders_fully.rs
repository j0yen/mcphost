//! PRD-mcphost-host-doctor-tool
//! AC3 -- Given every row of `DOCTOR_NEXT`, When rendered against the row's
//! fixture summary, Then no `{` remains and the output is <= 240 chars.

use mcphost::doctor::{DOCTOR_FIXTURES, DOCTOR_NEXT, family_of, parse_summary, render_next};

#[test]
fn every_row_has_a_fixture_and_renders_with_no_placeholder_left() {
    for (family, template) in DOCTOR_NEXT {
        let (_, tool, summary) = DOCTOR_FIXTURES
            .iter()
            .find(|(f, _, _)| f == family)
            .unwrap_or_else(|| panic!("DOCTOR_NEXT row {family:?} has no DOCTOR_FIXTURES entry"));
        let parsed = parse_summary(Some(summary), "unknown");
        assert_eq!(family_of(&parsed), *family, "the {family} fixture must classify as {family}: {summary:?}");
        let next = render_next(family, tool, &parsed);
        assert!(!next.contains('{') && !next.contains('}'), "{family}: placeholder left in {next:?}");
        assert!(next.chars().count() <= 240, "{family}: {} chars: {next}", next.chars().count());
        assert!(next.contains(tool), "{family}: next must name the tool: {next}");
        assert_ne!(next, *template, "{family}: rendered output equals the bare template");
    }
}

#[test]
fn every_fixture_belongs_to_a_row_and_a_bare_summary_still_renders_clean() {
    for (family, _, _) in DOCTOR_FIXTURES {
        assert!(DOCTOR_NEXT.iter().any(|(f, _)| f == family), "fixture {family:?} has no DOCTOR_NEXT row");
    }
    // A summary that carries none of the template's fields (and an
    // over-long tool name) still renders without `{` and within the cap.
    let long_tool = "t".repeat(300);
    for (family, _) in DOCTOR_NEXT {
        let next = render_next(family, &long_tool, &parse_summary(None, "tool_exception"));
        assert!(!next.contains('{'), "{family}: {next:?}");
        assert!(next.chars().count() <= 240, "{family}: {} chars", next.chars().count());
    }
}
