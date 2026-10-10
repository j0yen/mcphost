//! PRD-mcphost-unknown-import-domain-hint
//! AC3 (P0) -- Given own domain `example.test`, When `import example_test`
//! is published, Then the clause says `'example.test' is the server's
//! address` -- proving the domain is not a literal.

use crate::common;
use common::{TestServer, python_kind_registry_with_domain, signup};
use serde_json::json;

#[tokio::test]
async fn clause_names_the_configured_domain_not_a_literal() {
    let envs_dir = common::TempDataDir::new();
    let server =
        TestServer::start_with_kinds(python_kind_registry_with_domain(&envs_dir.0, "example.test")).await;
    let (_ns, key) = signup(&server.base_url, "Domhint AC3 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "domhint_probe",
                "kind": "python",
                "spec": {"source": "import example_test\ndef main(a):\n    return {}\n"},
            }),
        )
        .await
        .expect_err("the staging host's own domain is not a module");
    assert_eq!(err.data["error_code"], json!("unknown_import"));
    assert!(err.message.contains("'example.test' is the server's address"), "{}", err.message);
    assert!(!err.message.contains("mcphost.dev"), "{}", err.message);
    assert_eq!(err.data["own_domain"], json!("example.test"));

    // `mcphost_dev` is just an unknown module on this host: no clause.
    let other = client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "domhint_probe2",
                "kind": "python",
                "spec": {"source": "import mcphost_dev\ndef main(a):\n    return {}\n"},
            }),
        )
        .await;
    if let Err(e) = other {
        assert!(!e.message.contains("server's address"), "{}", e.message);
    }
}
