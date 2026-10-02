//! PRD-mcphost-sandbox-bridge-discoverability
//! AC1 (P0) -- Given a fresh tenant, When it calls `host.quickstart
//! kind=python`, Then the response carries `sandbox_api.import == "import
//! mcphost"` and `sandbox_api.modules` keys exactly equal to the runner
//! script's registered `mcphost.*` modules, and a fixture that adds a
//! module to the registration constant sees it in quickstart with no other
//! edit.

use crate::common;
use common::{TestServer, extract_structured, python_kind_registry, signup};
use mcphost::kinds::python::{BRIDGE_MODULES, BridgeModule, build_sandbox_api, runner_script_registered_modules};
use serde_json::json;
use std::collections::BTreeSet;

#[tokio::test]
async fn sandbox_api_import_and_modules_match_the_runner_scripts_own_registration() {
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Bridgedisc AC1 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let result = client
        .tools_call("host.quickstart", json!({"kind": "python"}))
        .await
        .expect("quickstart kind=python");
    let structured = extract_structured(&result);
    let sandbox_api = &structured["sandbox_api"];

    assert_eq!(
        sandbox_api["import"],
        json!("import mcphost"),
        "sandbox_api.import must read verbatim as the one import line: {sandbox_api}"
    );

    let modules = sandbox_api["modules"].as_object().expect("sandbox_api.modules is an object");
    let got_keys: BTreeSet<String> = modules.keys().cloned().collect();

    // The independent proof: parse PY_RUNNER_SCRIPT's own
    // `sys.modules["mcphost.<x>"] = ...` lines back out, rather than
    // re-reading BRIDGE_MODULES -- a real check that the two haven't
    // drifted, not a tautology.
    let registered: BTreeSet<String> = runner_script_registered_modules()
        .into_iter()
        .map(|name| format!("mcphost.{name}"))
        .collect();
    assert!(
        !registered.is_empty(),
        "runner_script_registered_modules() found no sys.modules[\"mcphost.*\"] lines -- parser broke"
    );
    assert_eq!(
        got_keys, registered,
        "sandbox_api.modules keys must exactly equal the runner script's own registered mcphost.* modules"
    );

    // Same keys BRIDGE_MODULES itself names -- quickstart's own source.
    let from_constant: BTreeSet<String> =
        BRIDGE_MODULES.iter().map(|m| format!("mcphost.{}", m.name)).collect();
    assert_eq!(got_keys, from_constant);
}

/// AC1's other half: "a fixture that adds a module to the registration
/// constant sees it in quickstart with no other edit". `build_sandbox_api`
/// is the exact function `control::quickstart` calls with `BRIDGE_MODULES`
/// -- calling it here with `BRIDGE_MODULES` plus one extra fixture entry
/// proves the quickstart-building code itself never special-cases a module
/// by name: the new module surfaces purely because it's in the slice, with
/// no other line of production code touched.
#[test]
fn a_module_added_to_the_registration_constant_appears_in_quickstart_with_no_other_edit() {
    let fixture = BridgeModule {
        name: "fixture_test_module",
        purpose: "a module that exists only to prove build_sandbox_api is generic",
        signatures: &["mcphost.fixture_test_module.ping() -- always returns true"],
    };
    let mut extended: Vec<BridgeModule> = BRIDGE_MODULES.to_vec();
    extended.push(fixture);

    let before = build_sandbox_api(BRIDGE_MODULES);
    let after = build_sandbox_api(&extended);

    assert!(
        before["modules"].get("mcphost.fixture_test_module").is_none(),
        "the fixture module must not already be present: {before}"
    );
    let after_modules = after["modules"].as_object().expect("modules object");
    assert_eq!(
        after_modules.get("mcphost.fixture_test_module"),
        Some(&json!(["mcphost.fixture_test_module.ping() -- always returns true"])),
        "the fixture module's signatures must appear verbatim: {after}"
    );
    // Every pre-existing module is still present and unchanged -- adding
    // one module never perturbs the others.
    for m in BRIDGE_MODULES {
        let key = format!("mcphost.{}", m.name);
        assert_eq!(after_modules.get(&key), before["modules"].get(&key), "module {key} must be unchanged");
    }
}
