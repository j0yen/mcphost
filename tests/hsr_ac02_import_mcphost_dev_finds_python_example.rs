//! PRD-mcphost-help-search-tool
//! AC2 (P0) — Given `q: "import mcphost.dev"`, When searched anonymously,
//! Then `hits[0].anchor` is `docs/kinds/python.md#…` and
//! `hits[0].example.code` contains the line `import mcphost` and no
//! `mcphost.dev`.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn import_mcphost_dev_first_hit_is_the_python_import_block() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let result = client
        .tools_call("host.help.search", json!({"q": "import mcphost.dev"}))
        .await
        .expect("host.help.search is callable anonymously");
    let body = extract_structured(&result);
    let first = &body["hits"][0];

    let anchor = first["anchor"].as_str().expect("hits[0].anchor");
    assert!(anchor.starts_with("docs/kinds/python.md#"), "hits[0].anchor was {anchor}: {body}");
    let code = first["example"]["code"].as_str().expect("hits[0].example.code");
    assert!(
        code.lines().any(|l| l.trim() == "import mcphost"),
        "example.code must carry the line `import mcphost`: {code}"
    );
    assert!(!code.contains("mcphost.dev"), "example.code must not mention mcphost.dev: {code}");
}
