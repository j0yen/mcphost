//! AC2 (P0) -- PRD-mcphost-resources-for-tenant-data. Given
//! `RESOURCE_TEMPLATES`, When `resources/templates/list` is sent keyless,
//! Then it lists exactly the const's rows with `uriTemplate` and `mimeType`;
//! Given `docs/protocol/resources.md`, Then its table equals the const
//! byte-for-byte.

use crate::common;
use common::{McpClient, TestServer};
use mcphost::resources::{
    RESOURCE_TEMPLATES, RESOURCES_TABLE_END, RESOURCES_TABLE_START, render_resources_doc, render_resources_table,
};
use serde_json::json;

#[tokio::test]
async fn mres_ac02_keyless_templates_list_is_exactly_the_const() {
    let server = TestServer::start().await;
    let reply = McpClient::new(&server.base_url).rpc("resources/templates/list", json!({})).await;
    let listed = reply["result"]["resourceTemplates"].as_array().unwrap_or_else(|| panic!("no templates: {reply}"));
    let got: Vec<(&str, &str)> = listed
        .iter()
        .map(|t| (t["uriTemplate"].as_str().expect("uriTemplate"), t["mimeType"].as_str().expect("mimeType")))
        .collect();
    let want: Vec<(&str, &str)> = RESOURCE_TEMPLATES.iter().map(|t| (t.uri_template, t.mime_type)).collect();
    assert_eq!(got, want, "{reply}");
    assert_eq!(want.len(), 4, "the PRD names four templates");
}

#[test]
fn mres_ac02_docs_table_equals_the_const_byte_for_byte() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/protocol/resources.md");
    let text = std::fs::read_to_string(&path).expect("read docs/protocol/resources.md");
    let start = text.find(RESOURCES_TABLE_START).expect("start marker");
    let end = text.find(RESOURCES_TABLE_END).expect("end marker") + RESOURCES_TABLE_END.len();
    assert_eq!(&text[start..end], render_resources_table(), "docs table drifted from RESOURCE_TEMPLATES");
    assert_eq!(text, render_resources_doc(), "run `mcphost resources-doc > docs/protocol/resources.md`");
}
