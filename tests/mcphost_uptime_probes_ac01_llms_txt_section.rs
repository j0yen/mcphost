//! PRD-mcphost-uptime-probes
//! AC1 — Given llms.txt, When the section is read, Then it shows the create
//! call and the status call, under 70 lines, with the calls/day arithmetic
//! (rewritten by PRD-mcphost-uptime-probe-recipe-green).

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
fn section_lists_the_one_create_call_and_the_status_call() {
    // PRD-mcphost-uptime-probe-recipe-green rewrote this section: one
    // `host.uptime.create` replaces the two tables, two tools and schedule.
    let section = section_lines().join("\n");

    assert_eq!(
        section.matches("host.uptime.create(").count(),
        1,
        "section must make exactly one create call: {section}"
    );
    assert!(
        section.contains("host.tool_call(name=\"status\")"),
        "section must show the status call: {section}"
    );
    assert!(
        !section.contains("host.tool_publish(name=") && !section.contains("host.state.table_create"),
        "section must not walk through tables or tool publishes any more: {section}"
    );
    assert!(
        section.contains("300"),
        "section must show the 300s free-plan floor: {section}"
    );
}

#[test]
fn section_states_the_calls_per_day_arithmetic() {
    let section = section_lines().join("\n");
    assert!(
        section.contains("288"),
        "section must state the calls/day at the free floor (288): {section}"
    );
}
