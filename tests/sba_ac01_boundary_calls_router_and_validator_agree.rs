//! PRD-mcphost-schema-bound-agreement
//! contract: reads registry handler::host_tool_descriptors
//! AC1 (P0) -- Given every validated descriptor in `host_tool_descriptors(kinds)`
//! and `admin_tool_descriptors()` (owner-registered args skipped), When each
//! schema-derived boundary call (empty array or string, 0, each declared
//! min/max, a max-length multibyte string) goes through `build_router`, Then
//! every `args`/`spec`-family reply (error registry) has a validator refusal or
//! a `SCHEMA_VALID_REFUSALS` entry.

use crate::common;
use crate::vbd_calls;
use common::{ADMIN_KEY, McpClient, TestServer, signup};
use mcphost::error_registry::lookup;
use mcphost::host_args::{arg_owners, owner_covers, SCHEMA_VALID_REFUSALS, UNVALIDATED, validate_host_args};
use mcphost::kinds::KindRegistry;
use serde_json::Value;
use std::time::Duration;

/// The keywords a boundary call can violate.
const BOUND_KEYWORDS: &[&str] = &["minItems", "maxItems", "minLength", "maxLength", "minimum", "maximum"];

const ARG_FAMILIES: &[&str] = &["args", "spec"];

/// A tool call's refusal as `(error code, message)`; `None` when it succeeded.
async fn refusal(client: &McpClient, tool: &str, args: &Value) -> Option<(String, String)> {
    match tokio::time::timeout(Duration::from_secs(20), client.tools_call(tool, args.clone())).await {
        Ok(Err(e)) => Some((e.error_code.unwrap_or_default(), e.message)),
        _ => None,
    }
}

async fn disagreements(admin_tools: bool) -> (usize, Vec<String>) {
    let kinds = KindRegistry::with_builtin();
    let server = TestServer::start_with_signup_rate_limit(100_000).await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let mut out = Vec::new();
    let mut checked = 0;
    for tool in vbd_calls::all_descriptors().into_iter().filter(|t| vbd_calls::is_admin(t) == admin_tools) {
        let name = tool.name.to_string();
        if UNVALIDATED.iter().any(|(t, _)| *t == name) || tool.meta.as_ref().is_some_and(|m| m.0.contains_key("alias_of")) {
            continue;
        }
        let base_args = vbd_calls::call_for(&tool.input_schema);
        let mut base: Option<Option<(String, String)>> = None;
        for (pointer, edge, args) in vbd_calls::boundary_calls(&tool.input_schema) {
            // An argument an owning module registered refuses (or clamps) itself.
            let owned = arg_owners().iter().any(|o| BOUND_KEYWORDS.iter().any(|k| owner_covers(o, &name, &pointer, k)));
            if owned {
                continue;
            }
            let verdict = validate_host_args(&kinds, &name, &args);
            let fixture;
            let client: &McpClient = if admin_tools {
                &admin
            } else {
                // A fresh tenant per call: a key- or tenant-destroying verb cannot disturb the next.
                let (_, key) = signup(&server.base_url, "SBA AC1").await;
                fixture = McpClient::with_bearer(&server.base_url, &key);
                &fixture
            };
            if base.is_none() {
                base = Some(refusal(client, &name, &base_args).await);
            }
            checked += 1;
            let Some((code, message)) = refusal(client, &name, &args).await else { continue };
            let family = lookup(&code).map(|r| r.family.as_str()).unwrap_or_default();
            let residue = SCHEMA_VALID_REFUSALS.iter().any(|r| r.tool == name && r.pointer == pointer);
            // Attributable to the edge: the refusal names the property the edge replaced, and the
            // placeholder call is either that very call or is not refused the same way.
            let property = pointer.trim_start_matches('/');
            let same_reply = base.as_ref().and_then(Option::as_ref) == Some(&(code.clone(), message.clone()));
            let attributable = (args == base_args || !same_reply) && message.contains(property);
            if ARG_FAMILIES.contains(&family) && verdict.is_ok() && code != "unknown_kind" && attributable && !residue {
                let shown: String = args.to_string().chars().take(60).collect();
                out.push(format!("{name} {pointer} ({edge}): {code}: {} -- {shown}", message.chars().take(110).collect::<String>()));
            }
        }
    }
    (checked, out)
}

async fn assert_agree(admin_tools: bool, floor: usize) {
    let (checked, found) = disagreements(admin_tools).await;
    println!("sba_ac01 admin={admin_tools}: {checked} boundary calls, {} disagreements", found.len());
    for line in &found {
        println!("DISAGREE {line}");
    }
    assert!(checked >= floor, "only {checked} boundary calls");
    assert!(found.is_empty(), "{} boundary calls where router and validator disagree:\n{}", found.len(), found.join("\n"));
}

#[tokio::test]
async fn sba_ac01_tenant_boundary_calls_router_and_validator_agree() {
    assert_agree(false, 100).await;
}

#[tokio::test]
async fn sba_ac01_admin_boundary_calls_router_and_validator_agree() {
    assert_agree(true, 5).await;
}
