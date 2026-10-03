//! PRD-mcphost-table-context-and-sql-passthrough
//! AC9 — Given `www/llms.txt`, When the database-in-a-minute section is
//! read, Then it is under 70 lines, shows create, append, one `model_set`
//! description, a GROUP BY and a LIKE question through `host.table.query`,
//! and `query_log`, and contains no `where` grammar and no claim that
//! `mcphost.state.query` is the only bridge path.

const LLMS_TXT: &str = include_str!("../www/llms.txt");

const SECTION_HEADING: &str = "## Give Claude a database in one minute";
const NEXT_HEADING: &str = "## Chart in a minute";

fn section_lines() -> Vec<&'static str> {
    let start = LLMS_TXT
        .find(SECTION_HEADING)
        .unwrap_or_else(|| panic!("www/llms.txt must have a '{SECTION_HEADING}' section"));
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
fn section_shows_table_create_and_append() {
    let section = section_lines().join("\n");
    assert!(section.contains("host.table.create"), "section must show host.table.create");
    assert!(section.contains("host.table.append"), "section must show host.table.append");
    assert!(section.contains("batches of 200"), "section must state the append batch size");
}

#[test]
fn section_shows_one_model_set_description_call() {
    let section = section_lines().join("\n");
    let calls = section.matches("host.table.model_set").count();
    assert_eq!(calls, 1, "section must show exactly one host.table.model_set call: {section}");
    assert!(
        section.contains("key=\"description\""),
        "the model_set call must set a description"
    );
}

#[test]
fn section_shows_a_group_by_and_a_like_question_through_table_query() {
    let section = section_lines().join("\n");
    assert!(
        section.contains("host.table.query(sql="),
        "section must ask questions through host.table.query"
    );
    assert!(
        section.to_uppercase().contains("GROUP BY"),
        "section must show a GROUP BY question"
    );
    assert!(section.contains("LIKE '%prod%'"), "section must show a LIKE question");
}

#[test]
fn section_shows_query_log() {
    let section = section_lines().join("\n");
    assert!(section.contains("host.table.query_log"), "section must show host.table.query_log");
}

#[test]
fn section_shows_the_desktop_connection_line() {
    let section = section_lines().join("\n");
    assert!(section.contains("Claude Desktop"), "section must show the Claude Desktop connection line");
    assert!(section.contains("mcpServers"), "the Desktop connection line must show how to add the MCP server");
}

#[test]
fn section_has_no_where_grammar_and_no_tool_publish_step() {
    let section = section_lines().join("\n");
    assert!(!section.contains("\"where\""), "section must not show the retired where grammar: {section}");
    assert!(!section.contains("host.tool_publish"), "section must not publish a tool: {section}");
    assert!(
        !section.contains("ALLOWED_OPS") && !section.contains("op\": \""),
        "section must not describe the retired six-operator grammar: {section}"
    );
}

#[test]
fn section_does_not_claim_state_query_is_the_only_bridge_path() {
    let section = section_lines().join("\n");
    assert!(
        !section.contains("only supported"),
        "section must not claim mcphost.state.query is the only bridge path: {section}"
    );
    assert!(
        section.contains("mcphost.table.query(sql)"),
        "section must name the mcphost.table.query(sql) bridge for python tool authors: {section}"
    );
}
