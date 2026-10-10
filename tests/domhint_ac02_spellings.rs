//! PRD-mcphost-unknown-import-domain-hint
//! AC2 (P0) -- Given specs with `import mcphost_dev`, `import mcphost.dev`,
//! and `import mcphost-dev` (in a string import), When published, Then all
//! three carry the domain clause; `import mcphost.devtools` does not.

use crate::common;
use common::{TestServer, python_kind_registry_with_domain, signup};
use serde_json::json;

const CLAUSE: &str = "'mcphost.dev' is the server's address, not a module";

async fn publish_error(source: &str) -> common::RpcError {
    let envs_dir = common::TempDataDir::new();
    let server =
        TestServer::start_with_kinds(python_kind_registry_with_domain(&envs_dir.0, "mcphost.dev")).await;
    let (_ns, key) = signup(&server.base_url, "Domhint AC2 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "domhint_probe", "kind": "python", "spec": {"source": source}}),
        )
        .await
        .expect_err("an unknown import must be refused")
}

#[tokio::test]
async fn underscore_dot_and_dash_spellings_carry_the_domain_clause() {
    for source in [
        "import mcphost_dev\ndef main(a):\n    return {}\n",
        "import mcphost.dev\ndef main(a):\n    return {}\n",
        "def main(a):\n    return {\"how\": \"import mcphost-dev\"}\n",
    ] {
        let err = publish_error(source).await;
        assert_eq!(err.data["error_code"], json!("unknown_import"), "{source}");
        assert!(err.message.contains(CLAUSE), "{source}: {}", err.message);
        assert_eq!(err.data["own_domain"], json!("mcphost.dev"), "{source}");
    }
}

#[tokio::test]
async fn a_longer_name_that_merely_starts_with_the_domain_has_no_clause() {
    let err = publish_error("import mcphost.devtools\ndef main(a):\n    return {}\n").await;
    assert_eq!(err.data["error_code"], json!("unknown_import"));
    assert!(!err.message.contains("server's address"), "{}", err.message);
    assert!(err.data.get("own_domain").is_none(), "{}", err.data);
}
