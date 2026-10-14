//! AC3 (PRD-mcphost-quickstart-by-client) — no usable `clientInfo` and
//! `User-Agent: codex-cli/1.2` detects `codex_cli` from the user agent.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn user_agent_fallback_detects_codex() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);
    let qs = extract_structured(
        &client
            .tools_call_with_header("host.quickstart", json!({}), ("user-agent", "codex-cli/1.2"))
            .await
            .expect("quickstart"),
    );
    assert_eq!(qs["client"]["detected"], "codex_cli", "{qs}");
    assert_eq!(qs["client"]["from"], "user_agent");
}
