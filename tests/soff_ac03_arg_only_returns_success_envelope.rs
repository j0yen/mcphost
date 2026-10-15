//! AC3 — `tenant_key` argument only (no `Authorization` header):
//! `host.self.offboard` returns the success envelope (unchanged behaviour,
//! pinned by this test).

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn soff_ac03_arg_only_returns_success_envelope() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Arg Only Leaver").await;
    // Anonymous client — auth is entirely via the `tenant_key` argument.
    let client = McpClient::new(&server.base_url);

    let out = client
        .tools_call("host.self.offboard", json!({"tenant_key": key}))
        .await
        .expect("arg-only self_offboard must return the success envelope");
    let out = extract_structured(&out);
    assert_eq!(out["disabled"], json!(true), "got {out}");
    assert_eq!(out["disabled_reason"], json!("self_offboard"), "got {out}");
}
