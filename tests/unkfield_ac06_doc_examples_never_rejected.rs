//! PRD-mcphost-spec-unknown-field-rejection
//! AC6 (guardrail) — Given the example spec from every `docs/kinds/*.md`
//! page and from `host.quickstart` for each kind, When each is passed to
//! `host.spec_test`, Then none fails `unknown_spec_field`.
//!
//! `Kind::example()` is the single source both `docs/kinds/<kind>.md` and
//! `host.quickstart(kind)` are built from (`kinds::docs::parse_kind_doc`
//! for echo/http/python/wasm; chain's own hand-written `example()` mirrors
//! its doc page verbatim) -- exercising `Kind::example().spec` through
//! `host.spec_test` for every registered kind covers both halves of this
//! AC with one fixture per kind.

use std::collections::HashMap;
use std::sync::Arc;

use crate::common;
use common::{McpClient, TestServer, TempDataDir, signup};
use mcphost::kinds::chain::ChainKind;
use mcphost::kinds::http::{HttpKind, NameLookup};
use mcphost::kinds::python::PythonKind;
use mcphost::kinds::wasm::WasmKind;
use mcphost::kinds::KindRegistry;
use serde_json::json;

fn every_kind_registry(data_dir: &std::path::Path) -> KindRegistry {
    let mut kinds = KindRegistry::with_builtin(); // echo
    let lookup: Arc<dyn NameLookup> = Arc::new(common::FixedLookup(HashMap::new()));
    kinds.register(Arc::new(HttpKind::for_test("127.0.0.1", lookup)));
    kinds.register(Arc::new(PythonKind::new(data_dir)));
    kinds.register(Arc::new(WasmKind::new()));
    kinds.register(Arc::new(ChainKind));
    kinds
}

#[tokio::test]
async fn no_kinds_own_example_spec_fails_unknown_spec_field() {
    let envs_dir = TempDataDir::new();
    let registry = every_kind_registry(&envs_dir.0);
    // Collect each kind's own example (spec, call_args) before handing the
    // registry to the server -- `Kind::example()` is a cheap, synchronous,
    // read-only call.
    let examples: Vec<(&'static str, serde_json::Value, serde_json::Value)> = registry
        .all()
        .map(|k| {
            let ex = k.example();
            (k.name(), ex.spec, ex.call_args)
        })
        .collect();
    assert_eq!(
        examples.len(),
        5,
        "expected echo/http/python/wasm/chain, got {:?}",
        examples.iter().map(|(n, ..)| n).collect::<Vec<_>>()
    );

    let server = TestServer::start_with_kinds(registry).await;
    let (_ns, key) = signup(&server.base_url, "Unkfield AC6").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    for (kind_name, spec, call_args) in examples {
        let result = client
            .tools_call(
                "host.spec_test",
                json!({"kind": kind_name, "spec": spec, "invocations": [call_args]}),
            )
            .await;
        if let Err(err) = result {
            assert_ne!(
                err.error_code.as_deref(),
                Some("unknown_spec_field"),
                "{kind_name}'s own example spec must never fail unknown_spec_field: {err:?}"
            );
        }
    }
}
