//! PRD-mcphost-contract-output-schemas-b
//! AC1 (P0) -- Given a captured response for `host.whoami` from each of
//! tenant, admin and implicit connections, When validated, Then all three
//! pass the single declared schema.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

async fn whoami(client: &McpClient) -> Value {
    extract_structured(&client.tools_call("host.whoami", json!({})).await.expect("host.whoami must succeed"))
}

#[tokio::test]
async fn tenant_admin_and_implicit_whoami_all_validate_against_the_declared_schema() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Outsch AC5").await;

    let tenant = whoami(&McpClient::with_bearer(&server.base_url, &key)).await;
    let admin = whoami(&McpClient::with_bearer(&server.base_url, ADMIN_KEY)).await;
    let implicit = whoami(&McpClient::new(&server.base_url)).await;

    // The one schema `tools/list` advertises for the tool.
    let listing = McpClient::with_bearer(&server.base_url, &key).tools_list().await.expect("tools/list");
    let advertised = listing["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|t| t["name"] == "host.whoami")
        .expect("host.whoami listed")["outputSchema"]
        .clone();
    assert_eq!(
        advertised,
        Value::Object((*mcphost::output_schemas::output_schema_for("host.whoami").expect("registered")).clone())
    );
    let validator = jsonschema::validator_for(&advertised).expect("schema compiles");

    for (label, response) in [("tenant", &tenant), ("admin", &admin), ("implicit", &implicit)] {
        let errors: Vec<String> = validator.iter_errors(response).map(|e| e.to_string()).collect();
        assert!(errors.is_empty(), "{label} whoami rejected by the declared schema: {errors:?}\n{response}");
    }
    // A shape that is not a whoami answer is rejected, so the schema is not vacuous.
    assert!(!validator.is_valid(&json!({"tenant": "x"})));
}
