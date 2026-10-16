//! PRD-mcphost-notes-to-future-self-b AC3 (P0) -- Given `host.quickstart` for
//! a free and a pro tenant, When `limits.plan` is read, Then `notes_max` is
//! 200 and 5000 and both equal the plan struct (drift test).

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup, signup_and_make_pro};
use mcphost::plans::PlanCatalog;
use serde_json::json;

#[tokio::test]
async fn nfsb_ac03() {
    let server = TestServer::start().await;
    let catalog = PlanCatalog::default_catalog();
    let free_struct = catalog.get("free").expect("free plan").notes_max;
    let pro_struct = catalog.get("pro").expect("pro plan").notes_max;

    let (_ns, free_key) = signup(&server.base_url, "NFSB Free").await;
    let free = McpClient::with_bearer(&server.base_url, &free_key);
    let (_ns, pro_key, _id) = signup_and_make_pro(&server, "NFSB Pro", "cus_nfsb_pro").await;
    let pro = McpClient::with_bearer(&server.base_url, &pro_key);

    let free_q = extract_structured(&free.tools_call("host.quickstart", json!({"kind": "echo"})).await.expect("free qs"));
    let pro_q = extract_structured(&pro.tools_call("host.quickstart", json!({"kind": "echo"})).await.expect("pro qs"));
    assert_eq!(free_q["limits"]["plan"]["notes_max"], json!(200), "{free_q}");
    assert_eq!(pro_q["limits"]["plan"]["notes_max"], json!(5000), "{pro_q}");
    assert_eq!(free_q["limits"]["plan"]["notes_max"], json!(free_struct));
    assert_eq!(pro_q["limits"]["plan"]["notes_max"], json!(pro_struct));
}
