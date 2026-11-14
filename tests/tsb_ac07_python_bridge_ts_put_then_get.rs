//! contract: reads registry kinds::python::BRIDGE_MODULES
//! PRD-mcphost-timeseries-buckets
//! AC7 -- Given a python tool calling `mcphost.ts.put` then `mcphost.ts.get`,
//! When called via `host.tool_call`, Then the bucket it wrote back is returned
//! and `host.quickstart` sandbox help lists `mcphost.ts`.

use crate::common;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::{Value, json};
use std::time::Duration;

const TOOL: &str = r#"import mcphost

def main(args):
    t = args["t"]
    mcphost.ts.put("latency_ms", 100, ts=t, tags={"url": "a"})
    mcphost.ts.put("latency_ms", 300, ts=t + 60, tags={"url": "a"})
    mcphost.ts.put("latency_ms", 999, ts=t + 90, tags={"url": "b"})
    return mcphost.ts.get("latency_ms", "1h", from_=t - 60, to=t + 3600, tags={"url": "a"})
"#;

#[tokio::test]
async fn tsb_ac07_a_python_tool_puts_then_gets_its_own_bucket() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "tsb ac7").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    // `host.quickstart` sandbox help lists the module and its four functions.
    let quickstart = extract_structured(&client.tools_call("host.quickstart", json!({"kind": "python"})).await.expect("quickstart"));
    let module = &quickstart["sandbox_api"]["modules"]["mcphost.ts"];
    assert!(module.is_array(), "sandbox_api.modules lacks mcphost.ts: {}", quickstart["sandbox_api"]["modules"]);
    let signatures = module.to_string();
    for function in ["mcphost.ts.put", "mcphost.ts.get", "mcphost.ts.list", "mcphost.ts.trim"] {
        assert!(signatures.contains(function), "{function} missing from {signatures}");
    }

    client
        .tools_call("host.tool_publish", json!({"name": "tsprobe", "kind": "python", "spec": {"source": TOOL}}))
        .await
        .expect("publish tsprobe");
    let t = 1_700_000_000_i64;
    // Wait out the first-call environment build, then call it the way the PRD says.
    poll_until_ready(&client, &format!("{ns}.tsprobe"), json!({"t": t}), Duration::from_secs(30))
        .await
        .unwrap_or_else(|e| panic!("tsprobe: {} {}", e.code, e.message));
    let called = extract_structured(
        &client.tools_call("host.tool_call", json!({"name": "tsprobe", "args": {"t": t + 7200}})).await.expect("host.tool_call"),
    );
    let bucket_result: Value = ["result", "payload"].iter().find_map(|k| called.get(*k).filter(|r| r.get("buckets").is_some()).cloned()).unwrap_or(called.clone());
    let buckets = bucket_result["buckets"].as_array().unwrap_or_else(|| panic!("no buckets in {called}"));
    // The second call re-put the same points one more time; tag `a` holds 2 points per call.
    let total: i64 = buckets.iter().map(|b| b["count"].as_i64().unwrap()).sum();
    assert_eq!(total, 2, "only url=a points (tags filter), from the called window: {bucket_result}");
    assert_eq!(buckets.len(), 1, "{bucket_result}");
    let b = &buckets[0];
    assert_eq!((b["min"].clone(), b["max"].clone(), b["avg"].clone()), (json!(100.0), json!(300.0), json!(200.0)), "{b}");
    assert_eq!(b["ts"], json!(t + 7200 - 60), "bucket starts at from: {b}");
    assert_eq!(bucket_result["bucket_s"], json!(3600));
}
