//! PRD-mcphost-uptime-probes
//! AC1 — Given llms.txt, When the section is read, Then it lists the two
//! tables, two tools, and the schedule call, under 70 lines, with the
//! calls/day arithmetic.

const LLMS_TXT: &str = include_str!("../www/llms.txt");

const SECTION_HEADING: &str = "## Uptime probes with no server";

/// Unlike `mcphost_team_memory_ac01_llms_txt_section.rs`'s fixed
/// `NEXT_HEADING`, this section is appended at the very end of
/// `www/llms.txt` (after "## Full documentation") so inserting it can
/// never shift what an *existing* section's own hardcoded `NEXT_HEADING`
/// constant finds next -- there is no heading after this one to name.
fn section_lines() -> Vec<&'static str> {
    let start = LLMS_TXT
        .find(SECTION_HEADING)
        .expect("www/llms.txt must have a '## Uptime probes with no server' section");
    LLMS_TXT[start..].lines().collect()
}

#[test]
fn section_is_under_70_lines() {
    let lines = section_lines();
    assert!(
        lines.len() < 70,
        "the 'Uptime probes with no server' section must be under 70 lines, got {}",
        lines.len()
    );
}

#[test]
fn section_lists_the_two_tables_two_tools_and_schedule_call() {
    let section = section_lines().join("\n");

    // The two tables.
    let table_create_count = section.matches("host.state.table_create").count();
    assert_eq!(
        table_create_count, 2,
        "section must create exactly two tables (targets, checks), found {table_create_count}"
    );
    assert!(
        section.contains("\"targets\"") && section.contains("\"checks\""),
        "section must name both the `targets` and `checks` tables: {section}"
    );

    // The two tools.
    let publish_count = section.matches("host.tool_publish").count();
    assert_eq!(
        publish_count, 2,
        "section must publish exactly two tools (probe, status), found {publish_count}"
    );
    assert!(
        section.contains("\"probe\"") && section.contains("\"status\""),
        "section must name both `probe` and `status`: {section}"
    );

    // The schedule call.
    assert!(
        section.contains("host.trigger.set") && section.contains("kind=\"schedule\""),
        "section must show the schedule call: {section}"
    );
    assert!(
        section.contains("300") || section.contains("*/5"),
        "section must show the 300s / 5-minute free-plan floor: {section}"
    );
}

#[test]
fn section_states_the_calls_per_day_arithmetic() {
    let section = section_lines().join("\n");
    assert!(
        section.contains("288") && section.contains("500"),
        "section must state the calls/day arithmetic (288 of 500): {section}"
    );
}
