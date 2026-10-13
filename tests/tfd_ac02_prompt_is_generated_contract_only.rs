//! AC2 (P0) -- Given a configured drafter, When `host.tool_from_description
//! {description}` runs, Then the prompt contains every entry of
//! `BRIDGE_MODULES`, the kind's `KindExample.spec`, and the description, and
//! contains no other hand-written sandbox prose.

use crate::common;
use common::{McpClient, TestServer, python_kind_registry, signup};
use mcphost::drafter::{FakeDrafter, RESPONSE_FORMAT, TOOL_NAME_REGEX, load_fixture};
use mcphost::kinds::python::{BRIDGE_MODULES, unknown_import_hint};
use mcphost::sandbox;
use serde_json::json;
use std::path::Path;
use std::sync::Arc;

#[tokio::test]
async fn prompt_carries_only_generated_contract_text_and_the_description() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/draft");
    let fake = Arc::new(FakeDrafter::new(vec![load_fixture(&dir, "utc_time").expect("fixture")]));
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_drafter(python_kind_registry(&envs_dir.0), fake.clone()).await;
    let (_ns, key) = signup(&server.base_url, "TFD AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let description = "return the current UTC time as ISO 8601";
    client
        .tools_call("host.tool_from_description", json!({"description": description}))
        .await
        .expect("host.tool_from_description ok");

    let prompts = fake.prompts();
    assert_eq!(prompts.len(), 1, "one draft call for a first-try pass");
    let prompt = &prompts[0];
    let example = server.state.kinds.get("python").expect("python kind").example();

    for module in BRIDGE_MODULES {
        assert!(prompt.contains(&format!("mcphost.{}", module.name)), "prompt lacks mcphost.{}", module.name);
    }
    assert!(prompt.contains(&example.spec.to_string()), "prompt lacks KindExample.spec");
    assert!(prompt.contains(description), "prompt lacks the description");

    // Everything left once the generated parts, the section labels and the
    // reply-format line are removed must be empty: no other prose.
    let modules = BRIDGE_MODULES.iter().map(|m| format!("mcphost.{}", m.name)).collect::<Vec<_>>().join("\n");
    let mut rest = prompt.clone();
    for part in [
        unknown_import_hint(),
        modules,
        example.spec.to_string(),
        example.call_args.to_string(),
        TOOL_NAME_REGEX.to_string(),
        description.to_string(),
        RESPONSE_FORMAT.to_string(),
    ] {
        rest = rest.replacen(&part, "", 1);
    }
    for label in ["sandbox_api", "sandbox_modules", "example_spec", "example_call_args", "name_regex", "description"] {
        rest = rest.replacen(&format!("[{label}]"), "", 1);
    }
    assert!(rest.trim().is_empty(), "hand-written prose left in the prompt: {rest:?}");
}
