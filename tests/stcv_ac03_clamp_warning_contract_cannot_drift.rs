//! PRD-mcphost-sandbox-timeout-clamp-visible
//! AC3 (P0) -- the keys of a serialised `ClampWarning`, `ClampWarning::FIELDS`
//! and `host.tool.publish` `properties.warnings.items.properties` (and its
//! `required`) are the same set; `code.enum` is the code set; `tools/list`
//! returns that `outputSchema` for `host.tool_publish`. Removing any one key
//! fails the check.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::kinds::python::{ClampWarning, clamp_warnings};
use mcphost::plans::PlanCatalog;
use serde_json::{Value, json};
use std::collections::BTreeSet;

const RAW: &str = include_str!("../contracts/host-output-schemas.v1.json");

fn keys<'a>(it: impl IntoIterator<Item = &'a str>) -> BTreeSet<String> {
    it.into_iter().map(str::to_string).collect()
}

fn items_of(schema: &Value) -> &Value {
    &schema["properties"]["warnings"]["items"]
}

/// The agreement check, over any publish schema: Err names the drift.
fn check(schema: &Value, serialised: &BTreeSet<String>) -> Result<(), String> {
    let fields = keys(ClampWarning::FIELDS.iter().copied());
    if &fields != serialised {
        return Err(format!("FIELDS {fields:?} != serialised keys {serialised:?}"));
    }
    let items = items_of(schema);
    let props = keys(items["properties"].as_object().ok_or("no items.properties")?.keys().map(String::as_str));
    if props != fields {
        return Err(format!("items.properties {props:?} != FIELDS {fields:?}"));
    }
    let required = keys(items["required"].as_array().ok_or("no items.required")?.iter().filter_map(Value::as_str));
    if required != fields {
        return Err(format!("items.required {required:?} != FIELDS {fields:?}"));
    }
    let codes = keys(items["properties"]["code"]["enum"].as_array().ok_or("no code.enum")?.iter().filter_map(Value::as_str));
    if codes != keys(ClampWarning::CODES.iter().copied()) {
        return Err(format!("code.enum {codes:?} != CODES"));
    }
    Ok(())
}

#[tokio::test]
async fn stcv_ac03() {
    let catalog = PlanCatalog::default_catalog();
    let free = catalog.get("free").expect("free plan");
    let spec = json!({"source": "def main(args):\n    return {}\n", "timeout_s": 45});
    let warnings = clamp_warnings(&spec, free);
    assert_eq!(warnings.len(), 1);
    let serialised = keys(serde_json::to_value(&warnings[0]).expect("serialise").as_object().expect("object").keys().map(String::as_str));

    // The committed contract file names the same keys.
    let raw: Value = serde_json::from_str(RAW).expect("contract parses");
    let committed = &raw["host.tool.publish"];
    let raw_props = keys(items_of(committed)["properties"].as_object().expect("raw items.properties").keys().map(String::as_str));
    assert_eq!(raw_props, keys(ClampWarning::FIELDS.iter().copied()), "committed contract keys");

    // tools/list serves the loaded schema, and it agrees.
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "STCV AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let listing = client.tools_list().await.expect("tools/list");
    let tool = listing["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|t| t["name"] == "host.tool_publish")
        .expect("host.tool_publish listed");
    let schema = &tool["outputSchema"];
    check(schema, &serialised).expect("tools/list outputSchema agrees with ClampWarning");

    // Removing any one key from any of the three sources fails the check.
    for field in ClampWarning::FIELDS {
        let mut s = schema.clone();
        s["properties"]["warnings"]["items"]["properties"].as_object_mut().unwrap().remove(*field);
        assert!(check(&s, &serialised).is_err(), "dropping properties.{field} must fail");

        let mut s = schema.clone();
        let req = s["properties"]["warnings"]["items"]["required"].as_array_mut().unwrap();
        req.retain(|v| v != *field);
        assert!(check(&s, &serialised).is_err(), "dropping required {field} must fail");

        let mut ser = serialised.clone();
        ser.remove(*field);
        assert!(check(schema, &ser).is_err(), "dropping serialised key {field} must fail");
    }
}
