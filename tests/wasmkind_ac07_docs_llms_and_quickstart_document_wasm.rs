//! PRD-mcphost-wasm-kind
//! AC7 -- Given llms.txt and the served kind descriptors, When an agent
//! reads them, Then the wasm kind, its limits, and a sentence on choosing
//! wasm vs. python are present.

use crate::common;
use common::{extract_structured, signup, wasm_kind_registry};
use serde_json::json;

const WASM_DOC: &str = include_str!("../docs/kinds/wasm.md");
const LLMS_FULL: &str = include_str!("../www/llms-full.txt");

fn assert_documents_wasm_kind(haystack: &str, source: &str) {
    assert!(
        haystack.contains("wasm"),
        "{source} must mention the wasm kind"
    );
    assert!(
        haystack.contains("timeout_s") && haystack.contains("memory_mb"),
        "{source} must document the wasm kind's limits (timeout_s/memory_mb): {haystack}"
    );
    assert!(
        haystack.contains("Choose `wasm`") || haystack.contains("choose `wasm`"),
        "{source} must carry a sentence on choosing wasm over python"
    );
}

#[test]
fn docs_kinds_wasm_md_documents_the_kind_its_limits_and_the_choice_sentence() {
    assert_documents_wasm_kind(WASM_DOC, "docs/kinds/wasm.md");
}

#[test]
fn readme_kinds_section_includes_the_wasm_kind() {
    let rendered = mcphost::kinds::docs::render_readme_section();
    assert!(
        rendered.contains("### `wasm`"),
        "README's generated kinds section must include a wasm heading: {rendered}"
    );
}

#[test]
fn llms_full_txt_documents_the_wasm_kind_its_limits_and_the_choice_sentence() {
    assert_documents_wasm_kind(LLMS_FULL, "www/llms-full.txt");
}

#[test]
fn wasm_example_is_sourced_from_its_docs_file() {
    use mcphost::kinds::Kind;
    let doc = mcphost::kinds::docs::parse_kind_doc(WASM_DOC);
    let from_kind = mcphost::kinds::wasm::WasmKind::new().example();
    assert_eq!(from_kind.blurb, doc.blurb);
    assert_eq!(from_kind.spec, doc.spec);
    assert_eq!(from_kind.call_args, doc.call_args);
}

#[tokio::test]
async fn host_quickstart_serves_a_worked_wasm_example_live() {
    let server = common::TestServer::start_with_kinds(wasm_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Wasm AC7 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let result = client
        .tools_call("host.quickstart", json!({"kind": "wasm"}))
        .await
        .expect("host.quickstart(wasm) must succeed for a registered kind");
    let structured = extract_structured(&result);

    assert_eq!(structured["kind"], json!("wasm"));
    assert_eq!(
        structured["steps"][0]["arguments"]["spec"]["component"],
        json!("AGFzbQEAAAAA"),
        "the live quickstart example must come from the same docs/kinds/wasm.md file: {structured}"
    );
}
