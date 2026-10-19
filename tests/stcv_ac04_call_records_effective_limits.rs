//! PRD-mcphost-sandbox-timeout-clamp-visible
//! AC4 (P1) -- a free tenant's tool published with `timeout_s: 45`: its
//! `host.tool_call` carries `_meta.resources_used.timeout_s_effective` 30 and
//! `clamped` `["timeout_s"]`; `host.call_trace` returns `timings.limits`
//! (requested 45, effective 30, wall_s 32); `host.tool_run` on the same spec
//! reports the same two keys.

use crate::common;
use crate::ctrace;
use common::extract_structured;
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn stcv_ac04() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let _switch = ctrace::trace_switch().await;
    let fx = ctrace::start().await;
    let spec = json!({
        "source": "def main(args):\n    return {\"ok\": True}\n",
        "args_schema": {"type": "object"},
        "timeout_s": 45,
    });
    fx.publish_python_spec("clamped", spec).await;
    fx.warm("clamped", json!({})).await;

    // Cold and warm both record.
    for path in ["first", "second"] {
        let result = fx
            .client
            .tools_call("host.tool_call", json!({"name": "clamped", "args": {}}))
            .await
            .unwrap_or_else(|e| panic!("{path} call: {e:?}"));
        let used = &result["_meta"]["resources_used"];
        assert_eq!(used["timeout_s_effective"], json!(30), "{path}: {result}");
        assert_eq!(used["clamped"], json!(["timeout_s"]), "{path}: {result}");

        let call_id = extract_structured(&result)["call_id"].as_str().expect("call_id").to_string();
        let trace = fx.trace(&fx.client, &call_id).await.expect("host.call_trace");
        let limits = &trace["timings"]["limits"];
        assert_eq!(limits["timeout_s_requested"], json!(45), "{path}: {trace}");
        assert_eq!(limits["timeout_s_effective"], json!(30), "{path}: {trace}");
        assert_eq!(limits["wall_s"], json!(32), "{path}: {trace}");
        assert_eq!(limits["clamped"], json!(["timeout_s"]), "{path}: {trace}");
    }

    let run = fx
        .client
        .tools_call("host.tool_run", json!({"name": "clamped", "args": {}}))
        .await
        .expect("tool_run");
    let used = &run["_meta"]["resources_used"];
    assert_eq!(used["timeout_s_effective"], json!(30), "{run}");
    assert_eq!(used["clamped"], json!(["timeout_s"]), "{run}");
}
