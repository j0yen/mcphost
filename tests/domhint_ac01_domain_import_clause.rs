//! PRD-mcphost-unknown-import-domain-hint
//! AC1 (P0) -- Given a python spec containing `from mcphost.dev import
//! table`, When published with own domain `mcphost.dev`, Then the error is
//! `unknown_import` and its message begins `unknown_import: no module
//! 'mcphost.dev'; 'mcphost.dev' is the server's address, not a module; the
//! sandbox API is 'import mcphost' (` and `data.own_domain` is
//! `mcphost.dev`.

use crate::common;
use common::{TestServer, python_kind_registry_with_domain, signup};
use serde_json::json;

const SOURCE: &str = "from mcphost.dev import table\ndef main(a):\n    return {}\n";

#[tokio::test]
async fn from_mcphost_dev_import_is_refused_with_the_domain_clause() {
    let envs_dir = common::TempDataDir::new();
    let server =
        TestServer::start_with_kinds(python_kind_registry_with_domain(&envs_dir.0, "mcphost.dev")).await;
    let (_ns, key) = signup(&server.base_url, "Domhint AC1 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "domhint_probe", "kind": "python", "spec": {"source": SOURCE}}),
        )
        .await
        .expect_err("a source importing the server's domain must be refused");
    assert_eq!(err.data["error_code"], json!("unknown_import"));
    let message = err.message.to_string();
    let want = "unknown_import: no module 'mcphost.dev'; 'mcphost.dev' is the server's address, not a module; the sandbox API is 'import mcphost' (";
    assert!(message.contains(want), "message must carry the domain clause: {message}");
    assert_eq!(err.data["own_domain"], json!("mcphost.dev"));
    assert_eq!(err.data["module"], json!("mcphost.dev"));
}
