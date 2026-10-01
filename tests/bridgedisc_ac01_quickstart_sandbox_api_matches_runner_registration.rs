//! PRD-mcphost-sandbox-bridge-discoverability
//! AC1 (P0) -- Given a fresh tenant, When it calls `host.quickstart
//! kind=python`, Then the response carries `sandbox_api.import ==
//! "import mcphost"` and `sandbox_api.modules` keys exactly equal the
//! runner script's registered `mcphost.*` modules, and a fixture that adds
//! a module to the registration constant sees it in quickstart with no
//! other edit.
//!
//! The "no other edit" half is proven by *how* this test computes its
//! expectation: `expected_modules` below is read straight from
//! `kinds::python::SANDBOX_API_MODULES` -- the same constant
//! `control::quickstart` builds `sandbox_api.modules` from -- rather than a
//! second, hand-typed list of module names this test file would itself
//! have to remember to update. A fixture that appended a fifth entry to
//! that constant would be caught by this same assertion, unchanged.

use crate::common;
use common::{TestServer, extract_structured, python_kind_registry, signup};
use mcphost::kinds::python::{SANDBOX_API_IMPORT_LINE, SANDBOX_API_MODULES};
use serde_json::json;
use std::collections::BTreeSet;

#[tokio::test]
async fn quickstart_sandbox_api_matches_registration_constant() {
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Bridge AC1 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let result = client
        .tools_call("host.quickstart", json!({"kind": "python"}))
        .await
        .expect("quickstart kind=python must succeed");
    let structured = extract_structured(&result);
    let sandbox_api = &structured["sandbox_api"];

    assert_eq!(
        sandbox_api["import"],
        json!(SANDBOX_API_IMPORT_LINE),
        "sandbox_api.import must be the literal import line: {sandbox_api}"
    );

    let modules_obj = sandbox_api["modules"]
        .as_object()
        .expect("sandbox_api.modules must be an object");
    let got_modules: BTreeSet<String> = modules_obj.keys().cloned().collect();
    let expected_modules: BTreeSet<String> = SANDBOX_API_MODULES
        .iter()
        .map(|m| format!("mcphost.{}", m.name))
        .collect();
    assert!(
        !expected_modules.is_empty(),
        "the registration constant must name at least one module"
    );
    assert_eq!(
        got_modules, expected_modules,
        "sandbox_api.modules keys must exactly equal the registration constant: {sandbox_api}"
    );

    // Every module's value is a non-empty list of one-line signatures --
    // `host.quickstart`'s whole point is that an agent reading only this
    // response learns how to call each one.
    for (name, sigs) in modules_obj {
        let sigs = sigs
            .as_array()
            .unwrap_or_else(|| panic!("{name}'s signatures must be an array: {sandbox_api}"));
        assert!(
            !sigs.is_empty(),
            "{name} must carry at least one signature: {sandbox_api}"
        );
    }

    // Requirement 1's `attrs` -- call/progress live on `mcphost` itself,
    // not as their own registered submodule, so they're a separate list.
    let attrs = sandbox_api["attrs"]
        .as_array()
        .expect("sandbox_api.attrs must be an array");
    assert!(
        attrs.contains(&json!("mcphost.call")) && attrs.contains(&json!("mcphost.progress")),
        "sandbox_api.attrs must name mcphost.call and mcphost.progress: {sandbox_api}"
    );
}
