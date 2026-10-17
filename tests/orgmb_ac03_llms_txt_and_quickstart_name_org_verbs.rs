//! PRD-mcphost-org-model-b
//! AC3 — Given `llms.txt` and `host.quickstart`, When the tool-name check
//! runs, Then all five `host.org.*` verbs appear and match `tools/list`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::kinds::KindRegistry;
use serde_json::json;
use std::collections::BTreeSet;
use std::path::Path;

#[tokio::test]
async fn orgmb_ac03() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Org Namer").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let listed: BTreeSet<String> = client
        .tools_list()
        .await
        .expect("tools/list")["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .filter(|n| n.starts_with("host.org."))
        .map(str::to_string)
        .collect();
    assert_eq!(listed.len(), 5, "tools/list must carry exactly five host.org.* verbs: {listed:?}");

    let quickstart = extract_structured(
        &client.tools_call("host.quickstart", json!({"kind": "echo"})).await.expect("host.quickstart"),
    );
    let in_quickstart: BTreeSet<String> = quickstart["org"]["tools"]
        .as_array()
        .expect("quickstart org.tools")
        .iter()
        .filter_map(|t| t.as_str().map(str::to_string))
        .collect();
    assert_eq!(in_quickstart, listed, "host.quickstart org.tools must equal tools/list");

    let generated: BTreeSet<String> = mcphost::llms_txt::tenant_tool_names(&KindRegistry::with_builtin())
        .into_iter()
        .filter(|n| n.starts_with("host.org."))
        .collect();
    assert_eq!(generated, listed, "llms.txt's generated names must equal tools/list");

    let committed = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("www/llms.txt"))
        .expect("www/llms.txt");
    for verb in &listed {
        assert!(committed.contains(&format!("- `{verb}`")), "www/llms.txt lacks {verb}");
    }
}
