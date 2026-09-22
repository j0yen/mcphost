//! PRD-mcphost-team-memory
//! AC1 — Given llms.txt, When the section is read, Then it lists the
//! table, two tools, group creation, two shares, and the add call, under
//! 70 lines.

const LLMS_TXT: &str = include_str!("../www/llms.txt");

const SECTION_HEADING: &str = "## Give your agents one memory";
const NEXT_HEADING: &str = "## Limits and pricing";

fn section_lines() -> Vec<&'static str> {
    let start = LLMS_TXT
        .find(SECTION_HEADING)
        .expect("www/llms.txt must have a '## Give your agents one memory' section");
    let end = LLMS_TXT[start..]
        .find(NEXT_HEADING)
        .map(|offset| start + offset)
        .unwrap_or(LLMS_TXT.len());
    LLMS_TXT[start..end].lines().collect()
}

#[test]
fn section_is_under_70_lines() {
    let lines = section_lines();
    assert!(
        lines.len() < 70,
        "the 'Give your agents one memory' section must be under 70 lines, got {}",
        lines.len()
    );
}

#[test]
fn section_lists_the_table_two_tools_group_and_two_shares_and_the_add_call() {
    let section = section_lines().join("\n");

    // The table.
    assert!(
        section.contains("host.table.create") && section.contains("\"memory\""),
        "section must show creating the `memory` table: {section}"
    );

    // The two tools.
    let publish_count = section.matches("host.tool_publish").count();
    assert_eq!(
        publish_count, 2,
        "section must publish exactly two tools (remember, recall), found {publish_count}"
    );
    assert!(
        section.contains("\"remember\"") && section.contains("\"recall\""),
        "section must name both `remember` and `recall`: {section}"
    );

    // Group creation.
    assert!(
        section.contains("host.group.create"),
        "section must create a group: {section}"
    );

    // The two shares.
    let share_count = section.matches("host.tool_share").count();
    assert_eq!(
        share_count, 2,
        "section must share exactly two tools to the group, found {share_count}"
    );

    // The add call.
    assert!(
        section.contains("host.group.add"),
        "section must add a teammate to the group: {section}"
    );
}

#[test]
fn section_states_what_writer_holds() {
    let section = section_lines().join("\n");
    let lower = section.to_ascii_lowercase();
    assert!(
        lower.contains("writer") && lower.contains("who"),
        "section must say what `writer` holds and name the `who` fallback \
         (mcphost does not expose the calling tenant's id to a shared python \
         tool yet): {section}"
    );
}
