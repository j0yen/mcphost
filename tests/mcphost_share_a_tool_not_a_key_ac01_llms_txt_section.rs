//! PRD-mcphost-share-a-tool-not-a-key
//! AC1 — Given llms.txt, When the section is read, Then it lists the six
//! calls in order with one argument block each and is under 60 lines.

const LLMS_TXT: &str = include_str!("../www/llms.txt");

const SECTION_HEADING: &str = "## Share a tool, not a key";
const NEXT_HEADING: &str = "### Share a tool, not a key: public variant";

/// The six calls, in the exact order Requirement 1 lists them.
const EXPECTED_CALLS_IN_ORDER: [&str; 6] = [
    "host.secret_set",
    "host.tool_publish",
    "host.group.create",
    "host.tool_share",
    "host.group.add",
    "host.tool_call",
];

fn section_lines() -> Vec<&'static str> {
    let start = LLMS_TXT
        .find(SECTION_HEADING)
        .expect("www/llms.txt must have a '## Share a tool, not a key' section");
    let end = LLMS_TXT[start..]
        .find(NEXT_HEADING)
        .map(|offset| start + offset)
        .unwrap_or(LLMS_TXT.len());
    LLMS_TXT[start..end].lines().collect()
}

#[test]
fn section_is_under_60_lines() {
    let lines = section_lines();
    assert!(
        lines.len() < 60,
        "the 'Share a tool, not a key' section must be under 60 lines, got {}",
        lines.len()
    );
}

#[test]
fn section_lists_the_six_calls_in_order() {
    let section = section_lines().join("\n");
    let mut last_pos = 0usize;
    for call in EXPECTED_CALLS_IN_ORDER {
        let pos = section[last_pos..].find(call).unwrap_or_else(|| {
            panic!("expected call `{call}` not found after position {last_pos} in section")
        });
        last_pos += pos + call.len();
    }
}

#[test]
fn each_call_has_exactly_one_fenced_argument_block() {
    let section = section_lines().join("\n");
    // Every numbered step's call sits inside its own ``` ... ``` block; a
    // fence count of 2*N confirms N complete, non-overlapping blocks (one
    // per call), not a stray unclosed fence.
    let fence_count = section.matches("```").count();
    assert_eq!(
        fence_count,
        EXPECTED_CALLS_IN_ORDER.len() * 2,
        "expected exactly one fenced argument block per call (six open+close pairs)"
    );

    for call in EXPECTED_CALLS_IN_ORDER {
        let call_pos = section
            .find(call)
            .unwrap_or_else(|| panic!("call `{call}` missing from section"));
        // The call name must itself appear inside a fenced block: an odd
        // number of ``` fences precede it.
        let preceding_fences = section[..call_pos].matches("```").count();
        assert_eq!(
            preceding_fences % 2,
            1,
            "call `{call}` must appear inside its own fenced argument block"
        );
    }
}
