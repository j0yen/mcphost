//! PRD-mcphost-schema-shape-agreement
//! contract: reads registry handler::host_tool_descriptors
//! AC1 (P0) -- Given every validated descriptor, When the generated test sets one
//! string leaf lacking `pattern`/`enum` to each `SHAPE_PROBE_STRINGS` value, or one
//! array lacking `uniqueItems` whose `maxItems` allows two to two equal items, via
//! `build_router`, Then every `args`/`spec` reply has a validator refusal or a
//! `SCHEMA_VALID_REFUSALS` entry with reason `referential` or `normalised`, else it
//! fails naming tool and pointer.

use crate::shape_probe;

use crate::common;
use crate::vbd_calls;
use common::{ADMIN_KEY, McpClient, TestServer, signup};
use mcphost::error_registry::lookup;
use mcphost::host_args::{SCHEMA_VALID_REFUSALS, UNVALIDATED, arg_owners, owner_covers};
use mcphost::kinds::KindRegistry;
use mcphost::precheck::precheck_args as validate_host_args;
use serde_json::{Value, json};
use shape_probe::{SHAPE_PROBE_STRINGS, SiteKind};
use std::time::Duration;

const ARG_FAMILIES: &[&str] = &["args", "spec"];

/// The residue reasons a shape probe may rely on: a refusal a schema cannot state.
fn shape_residue(tool: &str, pointer: &str) -> bool {
    SCHEMA_VALID_REFUSALS.iter().any(|r| {
        r.tool == tool
            && r.pointer == pointer
            && ["referential", "normalised"].iter().any(|k| r.reason == *k || r.reason.starts_with(&format!("{k}:")))
    })
}

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
        let schema = Value::Object((*tool.input_schema).clone());
        for site in shape_probe::sites(&schema) {
            let pointer = site.pointer();
            let top = format!("/{}", pointer.trim_start_matches('/').split('/').next().unwrap_or_default());
            // An argument an owning module registered refuses (or clamps) itself.
            if arg_owners().iter().any(|o| owner_covers(o, &name, &pointer, "*")) {
                continue;
            }
            let probes: Vec<(String, Value)> = match &site.kind {
                SiteKind::String => SHAPE_PROBE_STRINGS.iter().map(|s| (format!("{s:?}"), json!(s))).collect(),
                SiteKind::Dup { min, items } => vec![("two equal items".to_string(), Value::Array(vec![items.clone(); (*min).max(2)]))],
            };
            // A fresh tenant per site: a key- or tenant-destroying verb cannot disturb the next site.
            let fixture;
            let client: &McpClient = if admin_tools {
                &admin
            } else {
                let (_, key) = signup(&server.base_url, "SHP AC1").await;
                fixture = McpClient::with_bearer(&server.base_url, &key);
                &fixture
            };
            let base_args = vbd_calls::call_for(&tool.input_schema);
            let base_reply = refusal(client, &name, &base_args).await;
            for (label, value) in probes {
                let args = shape_probe::call_with(&tool.input_schema, &site, value);
                // A probe the schema already refuses is a validator refusal by construction.
                if validate_host_args(&kinds, &name, &args).is_err() {
                    continue;
                }
                checked += 1;
                let Some((code, message)) = refusal(client, &name, &args).await else { continue };
                let family = lookup(&code).map(|r| r.family.as_str()).unwrap_or_default();
                // Attributable to the probe: the refusal names the property and differs from the placeholder call's.
                let attributable = message.contains(&site.property()) && base_reply.as_ref() != Some(&(code.clone(), message.clone()));
                if ARG_FAMILIES.contains(&family) && code != "unknown_kind" && attributable && !shape_residue(&name, &top) && !shape_residue(&name, &pointer) {
                    let shown: String = args.to_string().chars().take(60).collect();
                    out.push(format!("{name} {pointer} ({label}): {code}: {} -- {shown}", message.chars().take(110).collect::<String>()));
                }
            }
        }
    }
    (checked, out)
}

async fn assert_agree(admin_tools: bool, floor: usize) {
    let (checked, found) = disagreements(admin_tools).await;
    println!("shp_ac01 admin={admin_tools}: {checked} shape probes, {} disagreements", found.len());
    for line in &found {
        println!("DISAGREE {line}");
    }
    assert!(checked >= floor, "only {checked} shape probes");
    assert!(found.is_empty(), "{} shape probes where router and validator disagree:\n{}", found.len(), found.join("\n"));
}

#[tokio::test]
async fn shp_ac01_tenant_shape_probes_router_and_validator_agree() {
    assert_agree(false, 100).await;
}

#[tokio::test]
async fn shp_ac01_admin_shape_probes_router_and_validator_agree() {
    assert_agree(true, 5).await;
}
