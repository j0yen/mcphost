//! PRD-mcphost-host-args-validated-by-default
//! AC4 (P0) -- Given every validated tool and every registered owner applying
//! to it, When the generated cross-contract test sends `fields: [x, x]`,
//! `fields: ["nope"]`, `limit: 5000` and one undeclared top-level key, Then each
//! reply is the owner's own outcome (`field_unknown` with `nearest`/`valid`;
//! success with `_meta.mcphost.limit_clamped`; `unknown_argument` with
//! `data.known`; the duplicate `fields` call succeeds with the field once) --
//! never a silent success for the unknown field or key, never `args_invalid`
//! with an owned `data.constraint`.

use crate::common;
use crate::vbd_calls;
use common::{ADMIN_KEY, McpClient, RpcError, TestServer, signup};
use mcphost::host_args::{VALIDATED_KEY, arg_owners};
use mcphost::paged::fields_for;
use serde_json::{Value, json};
use std::time::Duration;

const UNDECLARED: &str = "vbd_undeclared_key";

/// Verbs whose handler needs stored state the placeholder generator cannot
/// supply (an existing thread / channel): the owner's `fields` / `limit` parse
/// still runs first, and the handler then answers its own not-found, which is
/// accepted here for the duplicate-`fields` and `limit` calls.
const NEEDS_STATE: &[(&str, &str)] = &[
    ("host.msg.thread", "thread_not_found"),
    ("host.channel.read", "channel_not_found"),
];

fn needs_state(tool: &str, err: &RpcError) -> bool {
    NEEDS_STATE.iter().any(|(t, code)| *t == tool && err.error_code.as_deref() == Some(*code))
}

/// The owned keywords (any owner's), which an `args_invalid` must never carry.
fn owned_keyword(constraint: &str) -> bool {
    arg_owners().iter().any(|o| o.keywords.iter().any(|k| *k == "*" || *k == constraint))
}

fn refusal(err: &RpcError) -> String {
    format!("{} / {:?} / {}", err.message, err.error_code, err.data)
}

#[tokio::test]
async fn vbd_ac04_every_owner_answers_its_own_violation_on_every_tool_it_applies_to() {
    let server = TestServer::start_with_signup_rate_limit(100_000).await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let mut offenders: Vec<String> = Vec::new();
    let mut checked = 0usize;
    for tool in vbd_calls::all_descriptors() {
        if tool.input_schema.get(VALIDATED_KEY) != Some(&Value::Bool(true)) {
            continue;
        }
        let name = tool.name.to_string();
        let base = vbd_calls::call_for(&tool.input_schema);
        let tenant;
        let client = if vbd_calls::is_admin(&tool) {
            &admin
        } else {
            let (_ns, key) = signup(&server.base_url, "VBD AC4").await;
            tenant = McpClient::with_bearer(&server.base_url, &key);
            &tenant
        };
        for owner in arg_owners().iter().filter(|o| (o.applies)(&name)) {
            checked += 1;
            let call = |args: Value| async {
                tokio::time::timeout(Duration::from_secs(20), client.tools_call(&name, args))
                    .await
                    .unwrap_or_else(|_| panic!("{name}: timed out"))
            };
            let with = |extra: Value| {
                let mut args = base.clone();
                args.as_object_mut().expect("object").extend(extra.as_object().expect("object").clone());
                args
            };
            match owner.path {
                "/fields" => {
                    let field = fields_for(&name).and_then(|f| f.first().copied()).expect("a projecting verb has fields");
                    match call(with(json!({"fields": [field, field]}))).await {
                        Ok(_) => {}
                        Err(e) if needs_state(&name, &e) => {}
                        Err(e) => offenders.push(format!("({name}, /fields, duplicate) got {}", refusal(&e))),
                    }
                    match call(with(json!({"fields": ["nope"]}))).await {
                        Err(e) if e.error_code.as_deref() == Some("field_unknown") => {
                            if e.data.get("nearest").is_none() || !e.data["valid"].is_array() {
                                offenders.push(format!("({name}, /fields, nope) field_unknown without nearest/valid: {}", refusal(&e)));
                            }
                        }
                        other => offenders.push(format!("({name}, /fields, nope) got {other:?}")),
                    }
                }
                "/limit" => match call(with(json!({"limit": 5000}))).await {
                    Ok(v) => {
                        if v["_meta"]["mcphost"].get("limit_clamped").is_none() {
                            offenders.push(format!("({name}, /limit, 5000) succeeded without _meta.mcphost.limit_clamped: {v}"));
                        }
                    }
                    Err(e) if needs_state(&name, &e) => {}
                    Err(e) => offenders.push(format!("({name}, /limit, 5000) got {}", refusal(&e))),
                },
                "" => match call(with(json!({UNDECLARED: 1}))).await {
                    Err(e) if e.error_code.as_deref() == Some("unknown_argument") => {
                        let known: Vec<&str> = e.data["known"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                        let declared: Vec<&String> = tool.input_schema["properties"].as_object().expect("properties").keys().collect();
                        if known.len() != declared.len() || !declared.iter().all(|d| known.contains(&d.as_str())) {
                            offenders.push(format!("({name}, unknown key) data.known {known:?} != properties {declared:?}"));
                        }
                    }
                    other => offenders.push(format!("({name}, unknown key) got {other:?}")),
                },
                // A plan-owned ceiling: refused `budget_ceiling_exceeded` (or, for the
                // dry-run `host.estimate`, reported as that refusal), never `args_invalid`.
                "/budget" => {
                    let published = client
                        .tools_call("host.tool_publish", json!({"name": "vbdecho", "kind": "echo", "spec": {"schema": {"type": "object"}}}))
                        .await;
                    let args = json!({"name": "vbdecho", "args": {}, "budget": {"max_tool_latency_ms": 180_000_000}});
                    let code = match call(args).await {
                        Err(e) => e.error_code.unwrap_or_default(),
                        Ok(v) => v["structuredContent"]["refusal"]["code"].as_str().unwrap_or("success").to_string(),
                    };
                    if !owner.codes.contains(&code.as_str()) {
                        offenders.push(format!("({name}, /budget) got `{code}` (publish: {})", published.is_ok()));
                    }
                }
                "/invocations" => {
                    let args = json!({"kind": "echo", "spec": {"schema": {"type": "object"}}, "invocations": vec![json!({}); 6]});
                    match call(args).await {
                        Err(e) if owner.codes.contains(&e.error_code.as_deref().unwrap_or_default()) => {}
                        other => offenders.push(format!("({name}, /invocations) got {other:?}")),
                    }
                }
                other => offenders.push(format!("({name}) owner path {other:?} has no generated case: add one")),
            }
        }
    }
    // Never `args_invalid` carrying an owned keyword is covered above: each
    // expected reply is the owner's own code, so any other reply is named.
    assert!(owned_keyword("maximum") && owned_keyword("additionalProperties"));
    assert!(checked > 280, "only {checked} (tool, owner) pairs were checked");
    assert!(offenders.is_empty(), "{} owner violations:\n{}", offenders.len(), offenders.join("\n"));
}
