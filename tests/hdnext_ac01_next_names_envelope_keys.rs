//! PRD-mcphost-host-doctor-next-names-envelope-key
//! AC1 -- the run-824 group renders no `rows` without envelope data, and
//! names `mcphost.state.query` and `["rows"]` with it.

use mcphost::doctor::{family_of, parse_summary, render_next};

const WITHOUT: &str = "tool_exception: KeyError: 0 at tool.py:3\nreturn {\"first\": env[0][\"name\"]}";
const WITH: &str = "tool_exception: KeyError: 0 -- mcphost.state.query returns a dict with keys [\"rows\"] at tool.py:3\n\
                    envelope: mcphost.state.query [\"rows\"]\nreturn {\"first\": env[0][\"name\"]}";

#[test]
fn hdnext_ac01() {
    let parsed = parse_summary(Some(WITHOUT), "tool_exception");
    assert_eq!(family_of(&parsed), "tool_exception");
    let next = render_next("tool_exception", "envidx", &parsed);
    assert!(!next.contains("rows"), "run 824 reproduction: {next}");

    let parsed = parse_summary(Some(WITH), "tool_exception");
    assert_eq!(family_of(&parsed), "tool_exception");
    assert_eq!(parsed.class.as_deref(), Some("KeyError"));
    assert_eq!(parsed.source_line.as_deref(), Some("return {\"first\": env[0][\"name\"]}"));
    let next = render_next("tool_exception", "envidx", &parsed);
    assert!(next.contains("mcphost.state.query"), "{next}");
    assert!(next.contains("[\"rows\"]"), "{next}");
    assert!(next.chars().count() <= mcphost::doctor::MAX_NEXT_CHARS, "{next}");
}
