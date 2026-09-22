//! PRD-mcphost-database-in-a-minute
//! AC1 — Given llms.txt, When the section is read, Then it shows
//! `table_create`, batched insert, the `query` publish, and the Desktop
//! connection line, under 70 lines.

const LLMS_TXT: &str = include_str!("../www/llms.txt");

const SECTION_HEADING: &str = "## Give Claude a database in one minute";
const NEXT_HEADING: &str = "## Billing (tool calls, no dashboard)";

fn section_lines() -> Vec<&'static str> {
    let start = LLMS_TXT.find(SECTION_HEADING).unwrap_or_else(|| {
        panic!("www/llms.txt must have a '{SECTION_HEADING}' section")
    });
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
        "the 'Give Claude a database in one minute' section must be under 70 lines, got {}",
        lines.len()
    );
}

#[test]
fn section_shows_table_create() {
    let section = section_lines().join("\n");
    assert!(
        section.contains("host.state.table_create"),
        "section must show host.state.table_create"
    );
}

#[test]
fn section_shows_a_batched_insert_with_batch_size_stated() {
    let section = section_lines().join("\n");
    assert!(
        section.contains("host.state.insert"),
        "section must show host.state.insert"
    );
    // Requirement 1: the batch size must be stated, not just "insert in
    // batches" with no number.
    assert!(
        section.contains("batches of 200"),
        "section must state the insert batch size"
    );
}

#[test]
fn section_shows_the_query_publish() {
    let section = section_lines().join("\n");
    assert!(
        section.contains("host.tool_publish(name=\"query\""),
        "section must show the query tool's host.tool_publish call"
    );
    assert!(
        section.contains("kind=\"python\""),
        "the query tool must be published as python kind"
    );
}

#[test]
fn section_shows_the_desktop_connection_line() {
    let section = section_lines().join("\n");
    assert!(
        section.contains("Claude Desktop"),
        "section must show the Claude Desktop connection line"
    );
    assert!(
        section.contains("mcpServers"),
        "the Desktop connection line must show how to add the MCP server"
    );
}
