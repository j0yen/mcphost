//! PRD-mcphost-schema-coercion-opt-in AC6 -- Given `host.tool_test` on an
//! opted-in tool with `"4"`, When dry-run, Then the response carries the
//! same `coerced` entry the real call carries, and a 1 000-call loop adds
//! <= 50 us mean per validation.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::kinds::KindRegistry;
use mcphost::kinds::coerce::check_args;
use serde_json::json;
use std::time::Instant;

#[tokio::test]
async fn dry_run_shows_the_same_coerced_entry_as_a_real_call() {
    let server = TestServer::start_with_kinds(KindRegistry::with_builtin()).await;
    let (ns, key) = signup(&server.base_url, "Coerce AC6").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "opt_in", "kind": "echo", "spec": {"schema": {
                "type": "object", "properties": {"n": {"type": "integer", "x-coerce": true}}}}}),
        )
        .await
        .expect("publish");

    let dry = extract_structured(
        &client.tools_call("host.tool_test", json!({"name": "opt_in", "args": {"n": "4"}})).await.expect("dry run"),
    );
    let real = extract_structured(&client.tools_call(&format!("{ns}.opt_in"), json!({"n": "4"})).await.expect("real"));
    assert_eq!(dry["coerced"], real["coerced"], "dry: {dry}\nreal: {real}");
    assert_eq!(dry["coerced"][0]["from"], json!("string"));
}

#[test]
fn coercion_adds_under_50_us_per_validation() {
    let schema = json!({"type": "object", "properties": {
        "n": {"type": "integer", "x-coerce": true}, "s": {"type": "string"}, "b": {"type": "boolean", "x-coerce": true}}});
    let args = json!({"n": "4", "s": "x", "b": "true"});
    let plain_args = json!({"n": 4, "s": "x", "b": true});
    // Baseline: the same compile+validate with nothing to coerce.
    let iters = 1000;
    let t = Instant::now();
    for _ in 0..iters {
        let v = jsonschema::validator_for(&schema).unwrap();
        assert!(v.validate(&plain_args).is_ok());
    }
    let base = t.elapsed();
    let t = Instant::now();
    for _ in 0..iters {
        let (out, coerced) = check_args(&schema, &args).expect("coerces");
        assert_eq!(out, plain_args);
        assert_eq!(coerced.len(), 2);
    }
    let with = t.elapsed();
    let added_us = with.saturating_sub(base).as_micros() as f64 / iters as f64;
    println!("coerce AC6: +{added_us:.1} us per validation");
    assert!(added_us <= 50.0, "added {added_us} us per validation");
}
