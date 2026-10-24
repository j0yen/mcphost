//! contract: reads registry handler::host_tool_descriptors
//! PRD-mcphost-tool-call-output-root-opaque
//! AC3 (P0) -- Given tenant result T = {"checked_at":"2026-10-22",
//! "enabled":"yes","count":"many","tz":{"name":"UTC"}}, When T is validated
//! (a) as `{"data": T}` plus the envelope keys (the shape prod emitted on
//! 10-21: error path `data/checked_at`) and (b) mixed at the root as a
//! forwarded result, for the call root and the test root with dry_run false,
//! Then every case validates and the R2 emitter-union test passes (union of
//! emitted keys equals the envelope const exactly).

use crate::common;
use common::{McpClient, TestServer, extract_structured, publish, signup};
use mcphost::handler::host_tool_descriptors;
use mcphost::kinds::KindRegistry;
use mcphost::output_schemas::{FORWARDING_ROOTS, ROOT_ENVELOPE};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

fn tenant_result() -> Value {
    json!({"checked_at": "2026-10-22", "enabled": "yes", "count": "many", "tz": {"name": "UTC"}})
}

fn advertised(tool: &str) -> jsonschema::Validator {
    let tools = host_tool_descriptors(&KindRegistry::with_builtin());
    let schema = tools
        .iter()
        .find(|t| t.name.as_ref() == tool)
        .and_then(|t| t.output_schema.as_ref())
        .unwrap_or_else(|| panic!("{tool} advertises an outputSchema"));
    jsonschema::validator_for(&Value::Object((**schema).clone())).expect("schema compiles")
}

/// The host-written envelope keys of `tool`'s root with `dry_run` false.
fn envelope(tool: &str) -> Map<String, Value> {
    let mut keys = Map::new();
    keys.insert("call_id".to_string(), json!("7"));
    if tool == "host.tool.test" {
        keys.insert("dry_run".to_string(), json!(false));
        keys.insert("verdict".to_string(), json!("pass"));
    }
    keys
}

#[test]
fn osroot_ac03_tenant_result_validates_under_data_and_mixed_at_the_root() {
    for tool in FORWARDING_ROOTS {
        let validator = advertised(tool);

        let mut under_data = envelope(tool);
        under_data.insert("data".to_string(), tenant_result());
        let mut mixed = envelope(tool);
        mixed.extend(tenant_result().as_object().expect("object").clone());
        mixed.insert("forwarded_to".to_string(), json!("host.state.get"));
        mixed.insert("client_tool".to_string(), json!("host_state_get"));

        for (what, instance) in [("under data", under_data), ("mixed at the root", mixed)] {
            let instance = Value::Object(instance);
            if let Some(e) = validator.iter_errors(&instance).next() {
                panic!("{tool} {what}: {}: {e}\n{instance}", e.instance_path);
            }
        }
    }
}

fn keys_of(value: &Value) -> BTreeSet<String> {
    value.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default()
}

#[tokio::test]
async fn osroot_ac03_emitted_root_keys_equal_the_envelope_const() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Osroot AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    publish(&client, "emit", "echo", json!({"schema": {"type": "object"}})).await;

    // `data` and `result` are the names the tenant's output rides under; the
    // echo tool returns its args, so they are the tenant's own keys here.
    let carriers = ["data", "result"];
    let args = json!({"data": tenant_result(), "result": tenant_result()});
    let call = advertised("host.tool.call");
    let test = advertised("host.tool.test");

    let own_call = extract_structured(&client.tools_call("host.tool.call", json!({"name": "emit", "args": args})).await.expect("own call"));
    let own_test = extract_structured(&client.tools_call("host.tool.test", json!({"name": "emit", "args": args})).await.expect("own test"));
    let direct = extract_structured(&client.tools_call("host.whoami", json!({})).await.expect("direct whoami"));
    let forwarded = extract_structured(&client.tools_call("host.tool.call", json!({"name": "host.whoami"})).await.expect("forwarded"));

    assert_eq!(forwarded["forwarded_to"], json!("host.whoami"));
    assert_eq!(forwarded["client_tool"], json!("host_whoami"));
    for (what, validator, result) in [("own call", &call, &own_call), ("forwarded", &call, &forwarded), ("own test", &test, &own_test)] {
        if let Some(e) = validator.iter_errors(result).next() {
            panic!("{what}: {}: {e}\n{result}", e.instance_path);
        }
    }

    // What each branch added on top of what the tenant's tool or the forwarded
    // verb returned by itself.
    let tenant_own: BTreeSet<String> = keys_of(&args).into_iter().filter(|k| !carriers.contains(&k.as_str())).collect();
    let verb_own = keys_of(&direct);
    let mut emitted: BTreeSet<String> = BTreeSet::new();
    emitted.extend(keys_of(&own_call).difference(&tenant_own).cloned());
    emitted.extend(keys_of(&own_test).difference(&tenant_own).cloned());
    emitted.extend(keys_of(&forwarded).difference(&verb_own).cloned());
    emitted.insert("client_tool".to_string()); // the verb also returns its own `client_tool`; the value is asserted above

    let want: BTreeSet<String> =
        ROOT_ENVELOPE.iter().map(|k| k.to_string()).chain(["forwarded_to".to_string(), "client_tool".to_string()]).collect();
    let unused: Vec<_> = want.difference(&emitted).collect();
    let unnamed: Vec<_> = emitted.difference(&want).collect();
    assert!(unused.is_empty() && unnamed.is_empty(), "envelope const names no branch emits: {unused:?}; emitted keys not in it: {unnamed:?}");
}
