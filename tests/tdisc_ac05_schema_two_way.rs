//! PRD-mcphost-trusted-discovery-catalog-receipts AC5 (P0) -- the committed
//! `host.catalog.search` output schema and the `CatalogResult` the host emits
//! agree in both directions at every level: every emitted key is declared,
//! every declared key is emitted, and the entry, `calls_30d` and `view`
//! objects set `additionalProperties: false`. Removing any emitted field,
//! nested or not, from the schema fails the check naming the key path.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup_plain};
use mcphost::sharing::{CatalogCalls, CatalogEntry, CatalogResult, CatalogView};
use serde_json::{Value, json};

/// Keys the tool-call wrapper adds to a result (never `catalog_search` itself).
const WRAPPER_ONLY: &[&str] = &["next", "onboarding"];

fn committed_schema() -> Value {
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/contracts/host-output-schemas.v1.json"))
        .expect("read the committed output schemas");
    let all: Value = serde_json::from_str(&text).expect("parse");
    all["host.catalog.search"].clone()
}

/// Compare `emitted` with `schema` at `path`, recursing into objects and array
/// items; returns one message per difference, each naming its key path.
fn drift(emitted: &Value, schema: &Value, path: &str, wrapper_only: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    match emitted {
        Value::Object(map) => {
            let props = schema["properties"].as_object().cloned().unwrap_or_default();
            for (key, value) in map {
                match props.get(key) {
                    None => out.push(format!("emitted but undeclared: {path}.{key}")),
                    Some(sub) => out.extend(drift(value, sub, &format!("{path}.{key}"), &[])),
                }
            }
            for key in props.keys() {
                if !map.contains_key(key) && !wrapper_only.contains(&key.as_str()) {
                    out.push(format!("declared but never emitted: {path}.{key}"));
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                out.extend(drift(item, &schema["items"], &format!("{path}[]"), &[]));
            }
        }
        _ => {}
    }
    out
}

/// The objects that must refuse unknown keys, by path.
fn closed_objects(schema: &Value) -> Vec<(&'static str, Value)> {
    vec![
        ("tools[]", schema["properties"]["tools"]["items"].clone()),
        ("tools[].calls_30d", schema["properties"]["tools"]["items"]["properties"]["calls_30d"].clone()),
        ("view", schema["properties"]["view"].clone()),
    ]
}

fn populated() -> Value {
    let entry = CatalogEntry {
        name: "ns.tool".into(),
        kind: "echo".into(),
        owner: "ns".into(),
        visibility: "public".into(),
        group: Some("g".into()),
        description: Some("d".into()),
        tags: vec!["t".into()],
        tested_version: Some(1),
        stale_test: false,
        test_verdict: Some("pass".into()),
        calls_30d: CatalogCalls { n: 2, ok: 1 },
    };
    serde_json::to_value(CatalogResult {
        tools: vec![entry],
        view: CatalogView { include_synthetic: false, owners: 1 },
    })
    .expect("serialize")
}

/// Every key path in `value` (objects only, arrays as `[]`), parent first.
fn key_paths(value: &Value, path: &str, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                out.push(format!("{path}.{k}"));
                key_paths(v, &format!("{path}.{k}"), out);
            }
        }
        Value::Array(items) => {
            for item in items {
                key_paths(item, &format!("{path}[]"), out);
            }
        }
        _ => {}
    }
}

/// `schema` with the property at `path` (as `key_paths` spells it) removed.
fn without(schema: &Value, path: &str) -> Value {
    let mut copy = schema.clone();
    let mut node = &mut copy;
    let segments: Vec<&str> = path.trim_start_matches('.').split('.').collect();
    for (i, seg) in segments.iter().enumerate() {
        let (key, array) = match seg.strip_suffix("[]") {
            Some(k) => (k, true),
            None => (*seg, false),
        };
        if i == segments.len() - 1 && !array {
            node["properties"].as_object_mut().expect("properties").remove(key);
            return copy;
        }
        node = &mut node["properties"][key];
        if array {
            node = &mut node["items"];
        }
    }
    copy
}

#[tokio::test]
async fn tdisc_ac05() {
    let schema = committed_schema();
    let emitted = populated();

    // both directions agree on the fully populated result
    let diffs = drift(&emitted, &schema, "", WRAPPER_ONLY);
    assert!(diffs.is_empty(), "schema and CatalogResult drifted: {diffs:#?}");

    // the three nested objects refuse unknown keys
    for (path, node) in closed_objects(&schema) {
        assert_eq!(node["additionalProperties"], json!(false), "{path} must set additionalProperties false");
    }

    // removing any emitted field from the schema fails, naming the path
    let mut paths = Vec::new();
    key_paths(&emitted, "", &mut paths);
    assert!(paths.contains(&".tools[].calls_30d.ok".to_string()) && paths.contains(&".view.owners".to_string()), "{paths:?}");
    for path in &paths {
        let reduced = without(&schema, path);
        let diffs = drift(&emitted, &reduced, "", WRAPPER_ONLY);
        let expected = format!("emitted but undeclared: {path}");
        assert!(diffs.contains(&expected), "removing {path} was not caught: {diffs:?}");
    }
    // ...and a declared field that is not emitted fails too
    let mut extra = schema.clone();
    extra["properties"]["view"]["properties"]["ghost"] = json!({"type": "string"});
    assert!(drift(&emitted, &extra, "", WRAPPER_ONLY).contains(&"declared but never emitted: .view.ghost".to_string()));

    // the real handler output (null group and description, a populated receipt) agrees too
    let server = TestServer::start().await;
    let (_ns, key) = signup_plain(&server.base_url, "Tdisc AC5 Owner").await;
    let owner = McpClient::with_bearer(&server.base_url, &key);
    owner
        .tools_call("host.tool_publish", json!({"name": "shape", "kind": "echo", "spec": {"schema": {"type": "object"}}}))
        .await
        .expect("publish");
    owner
        .tools_call("host.tool_share", json!({"name": "shape", "visibility": "public", "description": "tdisc ac5"}))
        .await
        .expect("share");
    let found = extract_structured(&owner.tools_call("host.catalog.search", json!({"q": "tdisc ac5"})).await.expect("search"));
    assert_eq!(found["tools"].as_array().map(Vec::len), Some(1), "{found}");
    // (the wrapper's own keys -- `next`, `onboarding` -- are not the result's)
    let result_only = json!({"tools": found["tools"], "view": found["view"]});
    let diffs = drift(&result_only, &schema, "", WRAPPER_ONLY);
    assert!(diffs.is_empty(), "live result drifted from the schema: {diffs:#?}");
    assert_eq!(found["view"], json!({"include_synthetic": false, "owners": 1}), "{found}");
}
