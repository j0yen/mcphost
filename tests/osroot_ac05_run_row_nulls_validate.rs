//! contract: reads registry handler::host_tool_descriptors
//! contract: reads contracts/host-output-observational-edits.v1.json
//! PRD-mcphost-tool-call-output-root-opaque
//! AC5 (P0) -- Given a run row with every Option None, When it is serialized,
//! Then it validates as runs.wait/get result and runs.list item, and every
//! add_null entry names a field the serialized row contains.

use mcphost::db::RunRow;
use mcphost::handler::host_tool_descriptors;
use mcphost::kinds::KindRegistry;
use mcphost::output_schemas::{EditKind, observational_edits};
use mcphost::runs::run_to_json;
use serde_json::{Value, json};

const RUN_TOOLS: [&str; 3] = ["host.runs.wait", "host.runs.get", "host.runs.list"];

fn bare_run() -> RunRow {
    RunRow {
        id: "r1".to_string(),
        tenant_id: 1,
        tool_name: "t".to_string(),
        trigger: "call".to_string(),
        trigger_ref: None,
        caller_tenant_id: None,
        status: "queued".to_string(),
        progress_json: None,
        result_ref: None,
        error_class: None,
        started_unix: None,
        finished_unix: None,
        duration_ms: None,
        deadline_s: None,
        attempt: 0,
        purged_unix: None,
        args_json: None,
        manual: false,
        test: false,
        message_id: None,
        counters_json: None,
        error_data_json: None,
        end_user_subject: None,
        end_user_issuer: None,
        end_user_method: None,
        parent_run_id: None,
        step_no: None,
        parent_tool: None,
        item_index: None,
    }
}

fn validator(tool: &str) -> jsonschema::Validator {
    let tools = host_tool_descriptors(&KindRegistry::with_builtin());
    let schema = tools
        .iter()
        .find(|t| t.name.as_ref() == tool)
        .and_then(|t| t.output_schema.as_ref())
        .unwrap_or_else(|| panic!("{tool} advertises an outputSchema"));
    jsonschema::validator_for(&Value::Object((**schema).clone())).expect("schema compiles")
}

#[test]
fn osroot_ac05_a_run_row_with_every_option_none_validates_on_all_three_run_verbs() {
    let row = run_to_json(&bare_run());
    let mut get = row.clone();
    get["children"] = json!([]);
    let list = json!({"runs": [row.clone()]});

    let mut problems = Vec::new();
    for (tool, instance) in [("host.runs.wait", &row), ("host.runs.get", &get), ("host.runs.list", &list)] {
        for e in validator(tool).iter_errors(instance) {
            problems.push(format!("{tool}: {}: {e}", e.instance_path));
        }
    }
    assert!(problems.is_empty(), "{}\n{row}", problems.join("\n"));
}

#[test]
fn osroot_ac05_every_run_add_null_entry_names_a_serialized_field() {
    let row = run_to_json(&bare_run());
    let fields = row.as_object().expect("object");
    let mut seen = 0;
    for entry in observational_edits().entries.iter().filter(|e| RUN_TOOLS.contains(&e.tool.as_str()) && e.edit == EditKind::AddNull) {
        seen += 1;
        let field = entry.pointer.rsplit('/').next().unwrap_or_default();
        assert!(fields.contains_key(field), "{} {}: `{field}` is not a field of the serialized run row", entry.tool, entry.pointer);
    }
    assert!(seen > 0, "no add_null entry for the run verbs");
}
