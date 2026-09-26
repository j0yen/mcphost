//! PRD-mcphost-plan-catalog-state-quota-defaults
//! AC7 (P1) — Given `www/llms.txt`, When the operator notes are read, Then
//! they name the defaulting behavior and the startup line.

#[test]
fn operator_notes_name_plans_toml_defaulting_and_the_startup_log_line() {
    let text = std::fs::read_to_string("www/llms.txt").expect("read www/llms.txt");
    let start = text.find("## Operator notes").expect("Operator notes section must exist");
    let rest = &text[start..];
    let end = rest[1..].find("\n## ").map(|i| i + 1).unwrap_or(rest.len());
    let section = &rest[..end];

    assert!(
        section.contains("plans.toml") && section.contains("default_catalog()"),
        "operator notes must name the defaulting behavior: {section}"
    );
    assert!(
        section.contains("plans.toml: plan=") && section.contains("defaulted=["),
        "operator notes must name the startup log line's shape: {section}"
    );
}
