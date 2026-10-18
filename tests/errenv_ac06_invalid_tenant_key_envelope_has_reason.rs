//! PRD-mcphost-error-envelope-schema-contract
//! AC6 (P0) -- Given a request with an invalid tenant key, When any verb is
//! called, Then the error envelope has the same shape with `data` an object
//! carrying `reason`.

use crate::common;
use common::{McpClient, TestServer, http_kind_registry};
use mcphost::output_schemas::{INVALID_TENANT_KEY, error_branch_for, registered_tools};
use serde_json::{Value, json};

#[tokio::test]
async fn errenv_ac06() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let anon = McpClient::new(&server.base_url);
    let mut checked = 0usize;
    let mut accepted: Vec<&str> = Vec::new();
    let mut problems: Vec<String> = Vec::new();
    for tool in registered_tools() {
        let err = match anon.tools_call(tool, json!({"tenant_key": INVALID_TENANT_KEY})).await {
            Ok(_) => {
                accepted.push(tool);
                continue;
            }
            Err(e) => e,
        };
        // A public verb that validates its own arguments before it looks at
        // any tenant key never reaches the key check.
        if err.error_code.as_deref() != Some("tenant_key_invalid") {
            accepted.push(tool);
            continue;
        }
        let envelope = json!({"code": err.code, "message": err.message, "data": err.data});
        let branch = error_branch_for(tool).expect("branch");
        let valid = jsonschema::validator_for(&branch).expect("compiles").is_valid(&envelope);
        let reason = envelope["data"].get("reason").and_then(Value::as_str);
        if !envelope["data"].is_object() || !valid || reason.is_none_or(str::is_empty) {
            problems.push(format!("{tool}: {envelope} (valid={valid})"));
        }
        checked += 1;
    }
    assert!(problems.is_empty(), "first failing verb: {}\n{}", problems[0], problems.join("\n"));
    assert!(checked > 200, "only {checked} verbs rejected the key; accepted: {accepted:?}");
    // The public (unauthenticated) verbs: a handful, not the surface.
    assert!(accepted.len() <= 8, "verbs that never reach the tenant-key check: {accepted:?}");
}
