//! PRD-mcphost-unknown-import-domain-hint
//! AC5 (P0) -- Given the drift test, When it parses the runner script and
//! the configured public URL, Then the module list and domain in the hint
//! equal those parsed values, and `bridgedisc_ac01` still passes.
//!
//! Neither side is read back from the constant it is checked against: the
//! module list comes from the `sys.modules["mcphost.*"]` lines of the runner
//! script text, the domain from `own_domain_from_url` of the configured
//! public URL.

use crate::common;
use mcphost::kinds::http::own_domain_from_url;
use mcphost::kinds::python::{runner_script_registered_modules, unknown_import_hint};
use mcphost::kinds::{Kind, network_kinds};
use std::collections::BTreeSet;

fn hint_modules(message: &str) -> BTreeSet<String> {
    let open = message.find("'import mcphost' (").expect("hint lists modules") + "'import mcphost' (".len();
    let close = open + message[open..].find(')').expect("module list closes");
    message[open..close].split(", ").map(str::to_string).collect()
}

#[test]
fn hint_module_list_and_domain_equal_the_parsed_sources() {
    let public_url = mcphost::registry_manifest::public_url_from_env();
    let parsed_domain = own_domain_from_url(&public_url);
    assert!(!parsed_domain.is_empty(), "configured public URL {public_url} has no host");
    let parsed_modules: BTreeSet<String> = runner_script_registered_modules()
        .into_iter()
        .map(|m| format!("mcphost.{m}"))
        .collect();
    assert!(!parsed_modules.is_empty(), "runner script parser found no modules");

    let data = common::TempDataDir::new();
    let (http, python) = network_kinds(&public_url, &data.0).expect("network kinds");
    // R1: both kinds hold the one string.
    assert_eq!(http.own_domain(), python.own_domain());
    assert_eq!(python.own_domain(), parsed_domain);

    // Publish a source naming the domain; read the domain and list from the
    // real error the python kind produces.
    let spec = serde_json::json!({"source": format!("import {}\ndef main(a):\n    return {{}}\n", parsed_domain.replace('.', "_"))});
    let errors = python.validate_all(&spec);
    let message = errors.first().expect("the domain import is refused").to_string();
    assert!(
        message.contains(&format!("'{parsed_domain}' is the server's address, not a module")),
        "{message}"
    );
    assert_eq!(hint_modules(&message), parsed_modules);
    assert_eq!(hint_modules(&unknown_import_hint()), parsed_modules);
}
