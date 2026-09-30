//! PRD-mcphost-unknown-kind-routes-to-recipe
//! AC5 (P0) -- Given the alias table, When the `kindroute` startup test
//! runs, Then every alias resolves to a kind in the registered list and
//! every `recipe.steps[].tool` is a registered tool.
//!
//! No server, no signup -- this is the startup sanity check requirement 1
//! names ("checked against that list at startup ... a test covers it"),
//! run directly against `KindRegistry`/`kinds::aliases`/
//! `handler::host_tool_descriptors`.

use std::sync::Arc;

use crate::common::TempDataDir;
use mcphost::kinds::KindRegistry;
use mcphost::kinds::aliases::{KIND_ALIASES, recipe_names, recipe_steps};
use mcphost::kinds::chain::ChainKind;
use mcphost::kinds::http::HttpKind;
use mcphost::kinds::python::PythonKind;
use mcphost::kinds::wasm::WasmKind;
use serde_json::json;

/// Every kind `main.rs` registers in production (same set AC4's test
/// builds) -- the registry the alias table's own `kind` field must resolve
/// against.
fn production_kinds_registry(data_dir: &std::path::Path) -> KindRegistry {
    let mut kinds = KindRegistry::with_builtin();
    let lookup: Arc<dyn mcphost::kinds::http::NameLookup> =
        Arc::new(FixedLookupEmpty);
    kinds.register(Arc::new(HttpKind::for_test("127.0.0.1", lookup)));
    kinds.register(Arc::new(PythonKind::new(data_dir)));
    kinds.register(Arc::new(ChainKind));
    kinds.register(Arc::new(WasmKind::new()));
    kinds
}

struct FixedLookupEmpty;
impl mcphost::kinds::http::NameLookup for FixedLookupEmpty {
    fn lookup(&self, host: String) -> mcphost::kinds::http::LookupFuture {
        Box::pin(async move {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("no fixture for {host}"),
            ))
        })
    }
}

#[test]
fn every_alias_resolves_to_a_registered_kind() {
    let data_dir = TempDataDir::new();
    let kinds = production_kinds_registry(&data_dir.0);
    let registered = kinds.names();

    assert!(!KIND_ALIASES.is_empty(), "alias table must not be empty");
    for alias in KIND_ALIASES {
        assert!(
            registered.contains(&alias.kind),
            "alias '{}' names kind '{}', which is not registered: {registered:?}",
            alias.alias,
            alias.kind
        );
    }
}

#[test]
fn every_recipe_steps_tool_is_a_registered_tool() {
    let data_dir = TempDataDir::new();
    let kinds = production_kinds_registry(&data_dir.0);
    let tool_names: Vec<String> = mcphost::handler::host_tool_descriptors(&kinds)
        .into_iter()
        .map(|t| t.name.to_string())
        .collect();

    let mut checked_any = false;
    for recipe in recipe_names() {
        let steps = recipe_steps(recipe, "my_tool", &json!({}));
        assert!(!steps.is_empty(), "recipe '{recipe}' has no steps");
        for step in &steps {
            checked_any = true;
            let tool = step["tool"].as_str().expect("step.tool is a string");
            assert!(
                tool_names.iter().any(|t| t == tool),
                "recipe '{recipe}' step names '{tool}', which is not a registered tool: \
                 {tool_names:?}"
            );
        }
    }
    assert!(checked_any, "no recipe steps were checked");
}
