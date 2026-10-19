//! PRD-mcphost-sandbox-timeout-clamp-visible
//! AC5 (P0) -- a free tenant's python specs with `timeout_s: 30` and with no
//! `timeout_s`: neither publish result has a `warnings` key (absent, not
//! `[]`); each call's `_meta.resources_used.clamped` is `[]` and
//! `timeout_s_effective` is 30; each `host.call_trace` `timings.limits.clamped`
//! is `[]`.

use crate::common;
use crate::ctrace;
use common::extract_structured;
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn stcv_ac05() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let _switch = ctrace::trace_switch().await;
    let fx = ctrace::start().await;
    let source = "def main(args):\n    return {\"ok\": True}\n";
    let specs = [
        ("at_ceiling", json!({"source": source, "args_schema": {"type": "object"}, "timeout_s": 30})),
        ("unset", json!({"source": source, "args_schema": {"type": "object"}})),
    ];
    for (name, spec) in specs {
        let published = fx
            .client
            .tools_call("host.tool_publish", json!({"name": name, "kind": "python", "spec": spec}))
            .await
            .unwrap_or_else(|e| panic!("publish {name}: {e:?}"));
        assert!(extract_structured(&published).get("warnings").is_none(), "{name}: no warnings key: {published}");

        fx.warm(name, json!({})).await;
        let result = fx
            .client
            .tools_call("host.tool_call", json!({"name": name, "args": {}}))
            .await
            .unwrap_or_else(|e| panic!("call {name}: {e:?}"));
        let used = &result["_meta"]["resources_used"];
        assert_eq!(used["clamped"], json!([]), "{name}: {result}");
        assert_eq!(used["timeout_s_effective"], json!(30), "{name}: {result}");

        let call_id = extract_structured(&result)["call_id"].as_str().expect("call_id").to_string();
        let trace = fx.trace(&fx.client, &call_id).await.expect("host.call_trace");
        assert_eq!(trace["timings"]["limits"]["clamped"], json!([]), "{name}: {trace}");
    }
}
