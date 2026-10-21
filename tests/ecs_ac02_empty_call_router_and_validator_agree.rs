//! PRD-mcphost-empty-call-refusal-stated-in-schema
//! contract: reads registry handler::host_tool_descriptors
//! AC2 (P0) -- Given every descriptor from `host_tool_descriptors(kinds)` and
//! `admin_tool_descriptors()`, When `{}` is dispatched through the production
//! `build_router` (fixture tenant; admin key for `admin.*`) and also run
//! through `validate_host_args(kinds, tool, {})`, Then a reply whose registry
//! family is `args` or `spec` has a validator `Err`, a validator `Ok` has
//! `handler_runs` incremented by one, and the printed disagreement set is
//! empty.

use crate::common;
use crate::vbd_calls;
use common::{ADMIN_KEY, McpClient, TestServer, signup};
use mcphost::host_args::{UNVALIDATED, handler_runs};
use mcphost::precheck::precheck_args as validate_host_args;
use mcphost::kinds::KindRegistry;
use serde_json::{Value, json};
use std::time::Duration;

/// The argument families a descriptor can state (`ARG_FAMILIES` of the
/// conformance harness).
const ARG_FAMILIES: &[&str] = &["args", "spec"];

async fn disagreements(admin_tools: bool) -> (usize, Vec<String>) {
    let kinds = KindRegistry::with_builtin();
    let server = TestServer::start_with_signup_rate_limit(100_000).await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let empty = json!({});
    let mut out = Vec::new();
    let mut checked = 0;
    for tool in vbd_calls::all_descriptors().into_iter().filter(|t| vbd_calls::is_admin(t) == admin_tools) {
        let name = tool.name.to_string();
        let canonical = tool
            .meta
            .as_ref()
            .and_then(|m| m.0.get("alias_of"))
            .and_then(Value::as_str)
            .unwrap_or(&name)
            .to_string();
        let fixture;
        // `{}` names no tenant, so an admin call is counted under "".
        let (ns, client): (String, &McpClient) = if admin_tools {
            (String::new(), &admin)
        } else {
            let (tenant_ns, key) = signup(&server.base_url, "ECS AC2").await;
            fixture = McpClient::with_bearer(&server.base_url, &key);
            (tenant_ns, &fixture)
        };
        let verdict = validate_host_args(&kinds, &name, &empty);
        let before = (handler_runs(&ns, &name), handler_runs(&ns, &canonical));
        let outcome = tokio::time::timeout(Duration::from_secs(20), client.tools_call(&name, empty.clone())).await;
        let after = (handler_runs(&ns, &name), handler_runs(&ns, &canonical));
        let ran = (after.0 - before.0) + if canonical != name { after.1 - before.1 } else { 0 };
        checked += 1;
        if let Ok(Err(e)) = &outcome {
            let family = e.data.get("family").and_then(Value::as_str).unwrap_or_default();
            // A tool that opts out of validation keeps its handler's refusal
            // (`UNVALIDATED`, each pinned by its own test), and `unknown_kind`
            // answers the fixture's kind registry (it holds only `echo`), not
            // the shape of the call.
            let opted_out = UNVALIDATED.iter().any(|(t, _)| *t == name);
            let registry_state = e.error_code.as_deref() == Some("unknown_kind");
            if ARG_FAMILIES.contains(&family) && verdict.is_ok() && !opted_out && !registry_state {
                out.push(format!("{name}: handler refused {{}} as {family}/{:?} but the validator accepts it ({})", e.error_code, e.message));
            }
        }
        if verdict.is_ok() && ran == 0 {
            out.push(format!("{name}: the validator accepts {{}} but the handler did not run"));
        }
    }
    (checked, out)
}

async fn assert_agree(admin_tools: bool, floor: usize) {
    let (checked, found) = disagreements(admin_tools).await;
    println!("ecs_ac02 admin={admin_tools}: {checked} descriptors, {} disagreements", found.len());
    for line in &found {
        println!("DISAGREE {line}");
    }
    assert!(checked >= floor, "only {checked} descriptors");
    assert!(found.is_empty(), "{} descriptors where router and validator disagree on `{{}}`:\n{}", found.len(), found.join("\n"));
}

#[tokio::test]
async fn ecs_ac02_tenant_descriptors_router_and_validator_agree_on_an_empty_call() {
    assert_agree(false, 200).await;
}

#[tokio::test]
async fn ecs_ac02_admin_descriptors_router_and_validator_agree_on_an_empty_call() {
    assert_agree(true, 50).await;
}
