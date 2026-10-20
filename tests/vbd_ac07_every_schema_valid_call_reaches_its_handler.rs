//! PRD-mcphost-host-args-validated-by-default
//! AC7 (P0) -- Given every descriptor `host_tools(kinds, true)` and
//! `admin_tool_descriptors()` return, When a schema-valid call built by the
//! `insch_ac05` placeholder generator is dispatched with a fixture tenant (admin
//! key for `admin.*`), Then `handler_runs` increments for every tool -- no
//! descriptor refuses a call its handler would have run -- and the test fails
//! naming the tool, the generated args and the refusal text otherwise.

use crate::common;
use crate::vbd_calls;
use common::{ADMIN_KEY, McpClient, TestServer, signup};
use mcphost::host_args::handler_runs;
use serde_json::Value;
use std::time::Duration;

/// The key `dispatch_admin_tool` counts an admin call under: the `tenant` (else
/// `prefix`) argument, empty for "every tenant".
fn admin_counted_for(args: &Value) -> String {
    args.get("tenant").or_else(|| args.get("prefix")).and_then(Value::as_str).unwrap_or_default().to_string()
}

async fn sweep(admin_tools: bool) -> (usize, Vec<String>) {
    let server = TestServer::start_with_signup_rate_limit(100_000).await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let mut offenders = Vec::new();
    let mut checked = 0;
    for tool in vbd_calls::all_descriptors().into_iter().filter(|t| vbd_calls::is_admin(t) == admin_tools) {
        let name = tool.name.to_string();
        let args = vbd_calls::call_for(&tool.input_schema);
        // A clone under another name of the same verb counts under the canonical.
        let canonical = tool
            .meta
            .as_ref()
            .and_then(|m| m.0.get("alias_of"))
            .and_then(Value::as_str)
            .unwrap_or(&name)
            .to_string();
        let fixture;
        let (ns, client): (String, &McpClient) = if admin_tools {
            (admin_counted_for(&args), &admin)
        } else {
            // A fixture tenant per tool: key- and tenant-destroying verbs cannot
            // disturb the next call.
            let (tenant_ns, key) = signup(&server.base_url, "VBD AC7").await;
            fixture = McpClient::with_bearer(&server.base_url, &key);
            (tenant_ns, &fixture)
        };
        let before = (handler_runs(&ns, &name), handler_runs(&ns, &canonical));
        let outcome = tokio::time::timeout(Duration::from_secs(20), client.tools_call(&name, args.clone())).await;
        let after = (handler_runs(&ns, &name), handler_runs(&ns, &canonical));
        checked += 1;
        if after.0 <= before.0 && after.1 <= before.1 {
            let refusal = match outcome {
                Ok(Err(e)) => format!("{} / {:?} / {}", e.message, e.error_code, e.data),
                Ok(Ok(_)) => "no refusal, but the handler counter did not move".to_string(),
                Err(_) => "timed out".to_string(),
            };
            offenders.push(format!("{name} {args}: {refusal}"));
        }
    }
    (checked, offenders)
}

#[tokio::test]
async fn vbd_ac07_every_schema_valid_tenant_call_reaches_its_handler() {
    let (checked, offenders) = sweep(false).await;
    assert!(checked >= 200, "only {checked} tenant descriptors");
    assert!(offenders.is_empty(), "{} descriptors refuse a call their handler would run:\n{}", offenders.len(), offenders.join("\n"));
}

#[tokio::test]
async fn vbd_ac07_every_schema_valid_admin_call_reaches_its_handler() {
    let (checked, offenders) = sweep(true).await;
    assert!(checked >= 50, "only {checked} admin descriptors");
    assert!(offenders.is_empty(), "{} descriptors refuse a call their handler would run:\n{}", offenders.len(), offenders.join("\n"));
}
