//! contract: reads registry handler::host_tool_descriptors
//! PRD-mcphost-send-email-from-tool
//! AC7 (P0) -- Given the `SendStatus` enum and `BRIDGE_MODULES`, When the
//! drift tests run, Then the tool description lists every status and
//! `runner_script_registered_modules` includes `email`.

use mcphost::email_send::SendStatus;
use mcphost::handler::host_tool_descriptors;
use mcphost::kinds::KindRegistry;
use mcphost::kinds::python::{BRIDGE_MODULES, runner_script_registered_modules, unknown_import_hint};
use serde_json::Value;

#[test]
fn emsnd_ac07_every_send_status_is_in_both_descriptions_and_the_output_schema() {
    let tools = host_tool_descriptors(&KindRegistry::with_builtin());
    assert!(!SendStatus::ALL.is_empty());
    for name in ["host.email.send", "host.email.sends"] {
        let tool = tools.iter().find(|t| t.name == name).unwrap_or_else(|| panic!("{name} is registered"));
        let description = tool.description.clone().unwrap_or_default();
        for status in SendStatus::ALL {
            assert!(description.contains(status.as_str()), "{name} description omits {}: {description}", status.as_str());
        }
    }
    // The row `status` the output schema declares is the enum, in order.
    let sends = tools.iter().find(|t| t.name == "host.email.sends").expect("host.email.sends");
    let schema = sends.output_schema.as_ref().expect("host.email.sends has an output schema");
    let declared: Vec<&str> = schema["properties"]["sends"]["items"]["properties"]["status"]["enum"]
        .as_array()
        .expect("status enum")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(declared, SendStatus::names());
}

#[test]
fn emsnd_ac07_the_runner_registers_the_email_module_the_constant_names() {
    let registered = runner_script_registered_modules();
    assert!(registered.iter().any(|m| m == "email"), "runner script registers: {registered:?}");
    let declared: Vec<&str> = BRIDGE_MODULES.iter().map(|m| m.name).collect();
    assert!(declared.contains(&"email"), "BRIDGE_MODULES names: {declared:?}");
    for module in BRIDGE_MODULES.iter().filter(|m| m.name != "elicit") {
        assert!(registered.iter().any(|r| r == module.name), "{} is declared but never registered", module.name);
    }
    assert!(unknown_import_hint().contains("mcphost.email"), "{}", unknown_import_hint());
}
