//! AC6 — Given a step whose error is `args_invalid` and a forced publish
//! listing it in `on`, When called, Then one attempt is made and no
//! `attempts` array has more than one entry.

use crate::common;
use common::{McpClient, TestServer, chain_kind_registry, publish, signup};
use serde_json::json;

#[tokio::test]
async fn args_invalid_listed_in_on_is_still_one_attempt() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Cretr AC6").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    publish(
        &client,
        "strict",
        "echo",
        json!({"schema": {"type": "object", "required": ["x"], "properties": {"x": {"type": "string"}}}}),
    )
    .await;

    // The honest publish path refuses `args_invalid` in `on`...
    let spec = |on_error: Option<&str>| {
        let mut step = json!({
            "tool": "strict",
            "args": {},
            "retry": {"max": 2, "backoff_ms": 10, "on": ["args_invalid"]},
        });
        if let Some(m) = on_error {
            step["on_error"] = json!(m);
        }
        json!({"steps": [step]})
    };
    let refused = client
        .tools_call("host.tool_publish", json!({"name": "honest", "kind": "chain", "spec": spec(None)}))
        .await
        .expect_err("args_invalid is not a retry family");
    assert_eq!(refused.error_code.as_deref(), Some("retry_on_unknown"));

    // ...so force it into the store, bypassing publish validation.
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns.clone())
        .await
        .unwrap()
        .expect("tenant");
    for (name, on_error) in [("forced_stop", None), ("forced_continue", Some("continue"))] {
        server
            .state
            .db
            .upsert_tool(tenant.id, name.to_string(), "chain".to_string(), spec(on_error), 5, vec![])
            .await
            .expect("forced upsert");
    }

    let err = client
        .tools_call(&format!("{ns}.forced_stop"), json!({}))
        .await
        .expect_err("the missing required arg fails the step");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{err:?}");
    assert!(err.data.get("attempts").is_none(), "no retry bookkeeping: {:?}", err.data);
    assert!(err.data.get("retry_exhausted").is_none(), "{:?}", err.data);

    // With on_error: continue the trace is returned, so the attempts array
    // is observable: exactly one entry.
    let out = client
        .tools_call(&format!("{ns}.forced_continue"), json!({}))
        .await
        .expect("a continue step does not fail the chain");
    let structured = common::extract_structured(&out);
    let attempts = structured["steps"][0]["attempts"].as_array().expect("attempts array");
    assert_eq!(attempts.len(), 1, "{structured}");
    assert_eq!(attempts[0]["error_code"], json!("args_invalid"));
}
