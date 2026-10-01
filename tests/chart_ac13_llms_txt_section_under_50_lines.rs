//! PRD-mcphost-chart-in-a-minute
//! AC13 — Given `www/llms.txt`, When the chart section is read, Then it
//! is under 50 lines and shows one chart call and one share link and no
//! grammar other than SQL.

const LLMS_TXT: &str = include_str!("../www/llms.txt");
const SECTION_HEADING: &str = "## Chart in a minute";

fn section_text() -> &'static str {
    let start = LLMS_TXT.find(SECTION_HEADING).expect("www/llms.txt must have a '## Chart in a minute' section");
    let rest = &LLMS_TXT[start..];
    let end = rest[SECTION_HEADING.len()..]
        .find("\n## ")
        .map(|off| SECTION_HEADING.len() + off)
        .unwrap_or(rest.len());
    &rest[..end]
}

#[test]
fn chart_section_is_under_50_lines_one_chart_call_one_share_link_sql_only() {
    let section = section_text();
    let line_count = section.lines().count();
    assert!(line_count < 50, "chart section is {line_count} lines, expected under 50:\n{section}");

    let chart_call_count = section.matches("host.table.chart(").count();
    assert_eq!(chart_call_count, 1, "expected exactly one host.table.chart( call: {section}");

    let share_link_count = section.matches("/charts/").count();
    assert_eq!(share_link_count, 1, "expected exactly one share link (/charts/...): {section}");

    // Requirement/AC13: "no grammar other than SQL" -- the section must
    // never show the database recipe's `where: [{col, op, value}]` filter
    // grammar (its one non-SQL query language), only `sql=`.
    assert!(!section.contains("\"where\""), "chart section must not show non-SQL query grammar: {section}");
    assert!(!section.contains("\"op\""), "chart section must not show non-SQL query grammar: {section}");
    assert!(section.contains("sql="), "chart section must show the sql= argument: {section}");
}
