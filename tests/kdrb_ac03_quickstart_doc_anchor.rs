//! PRD-mcphost-kind-docs-from-registry-b AC3: `host.quickstart kind=chain`
//! returns `doc_anchor` `docs/kinds/chain.md#spec-fields` and `spec_fields`
//! equal to the page's `kind-fields` names.

use crate::common::{McpClient, TestServer, chain_kind_registry, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn kdrb_ac03_quickstart_doc_anchor_and_spec_fields_match_the_page() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Kdrb AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let result = client.tools_call("host.quickstart", json!({"kind": "chain"})).await.expect("quickstart kind=chain");
    let structured = extract_structured(&result);

    assert_eq!(structured["doc_anchor"], json!("docs/kinds/chain.md#spec-fields"), "{structured}");

    let page = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/kinds/chain.md")).expect("read page");
    let block = page
        .split("<!-- kind-fields:start -->")
        .nth(1)
        .and_then(|s| s.split("<!-- kind-fields:end -->").next())
        .expect("kind-fields block");
    let page_names: Vec<String> = block
        .lines()
        .skip(2)
        .filter_map(|l| l.strip_prefix("| `")?.split('`').next().map(str::to_string))
        .collect();
    assert!(!page_names.is_empty(), "no field rows parsed from the page: {block}");
    let got: Vec<String> =
        structured["spec_fields"].as_array().expect("spec_fields array").iter().map(|v| v.as_str().unwrap().to_string()).collect();
    assert_eq!(got, page_names);
}
