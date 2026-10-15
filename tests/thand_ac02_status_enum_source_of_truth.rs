//! PRD-mcphost-task-handoff-tool AC2 (P0) — Given `host_tools()`, When the
//! drift test runs, Then `host.task.get`'s `status` enum equals
//! `TaskStatus::ALL` and llms.txt lists the same five words.

use mcphost::handler::host_tool_descriptors;
use mcphost::kinds::KindRegistry;
use mcphost::tasks::{STATUS_TOOLS, TaskStatus};

const LLMS_TXT: &str = include_str!("../www/llms.txt");
const SECTION: &str = "## Hand work to another agent";

fn all() -> Vec<String> {
    TaskStatus::ALL.iter().map(|s| s.as_str().to_string()).collect()
}

fn schema_statuses(tool_name: &str) -> Vec<String> {
    let tool = host_tool_descriptors(&KindRegistry::default())
        .into_iter()
        .find(|t| t.name.as_ref() == tool_name)
        .unwrap_or_else(|| panic!("{tool_name} is a host tool"));
    let schema = tool.output_schema.unwrap_or_else(|| panic!("{tool_name} has an output schema"));
    schema["properties"]["status"]["enum"]
        .as_array()
        .unwrap_or_else(|| panic!("{tool_name}: status is an enum"))
        .iter()
        .map(|v| v.as_str().expect("string").to_string())
        .collect()
}

/// The words on the section's `Statuses:` sentence, in order.
fn llms_statuses() -> Vec<String> {
    let start = LLMS_TXT.find(SECTION).expect("llms.txt has the task section");
    let body = &LLMS_TXT[start + SECTION.len()..];
    let body = &body[..body.find("\n## ").unwrap_or(body.len())];
    let after = &body[body.find("Statuses: ").expect("a `Statuses:` line") + "Statuses: ".len()..];
    let line = after.split(". ").next().expect("sentence");
    line.split(", ").map(|k| k.trim_matches('`').to_string()).collect()
}

fn agree(a: &[String], b: &[String], c: &[String]) -> bool {
    a == b && b == c
}

#[test]
fn the_get_schema_enum_and_llms_txt_equal_task_status_all() {
    let (all, schema, doc) = (all(), schema_statuses("host.task.get"), llms_statuses());
    assert_eq!(all.len(), 5, "five statuses: {all:?}");
    assert!(agree(&all, &schema, &doc), "ALL {all:?} / schema {schema:?} / llms.txt {doc:?} must be identical");
    assert!(LLMS_TXT.contains(&format!("Statuses: {}", TaskStatus::llms_txt_statuses())));
    for tool in STATUS_TOOLS {
        assert_eq!(schema_statuses(tool), all, "{tool} declares its own status set");
    }

    // Removing one status from any of the three breaks agreement.
    let drop = |v: &[String]| v[1..].to_vec();
    assert!(!agree(&drop(&all), &schema, &doc), "ALL missing a status must fail");
    assert!(!agree(&all, &drop(&schema), &doc), "schema missing a status must fail");
    assert!(!agree(&all, &schema, &drop(&doc)), "llms.txt missing a status must fail");
}

#[test]
fn the_get_description_names_every_status() {
    let tool = host_tool_descriptors(&KindRegistry::default())
        .into_iter()
        .find(|t| t.name.as_ref() == "host.task.get")
        .expect("host.task.get");
    let description = tool.description.expect("description");
    for status in all() {
        assert!(description.contains(&status), "description misses {status}: {description}");
    }
}
