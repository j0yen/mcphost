//! AC5 -- Given `panel_cost_optimizer_02`'s recorded publish shape with
//! `kind: http` requested, When replayed, Then the outcome is an http-kind
//! tool or a `kind_mismatch` error -- not a python-kind publish.
//!
//! The TL;DR: the persona's cold-start-benchmark task required an http-kind
//! tool but its own spec measured cold-start latency with inline Python
//! source (using stdlib timing, no `upstream`/`method`/`url`) -- exactly the
//! one-sided `python` signal `infer_kind_signal` detects. Replayed here with
//! `kind: http` requested, as the fix requires: never a silent python
//! publish.

use crate::common;
use common::{TempDataDir, TestServer, all_kinds_registry, signup};
use serde_json::json;

#[tokio::test]
async fn cold_start_benchmark_spec_with_kind_http_is_never_silently_python() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(all_kinds_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Kind Honor AC5").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    // The recorded incident's spec shape: inline Python measuring cold-start
    // latency and returning the required field name, no declarative http
    // fields at all.
    let cold_start_spec = json!({
        "source": "import time\n\
                   def main(args):\n    \
                       start = time.time()\n    \
                       elapsed_ms = (time.time() - start) * 1000\n    \
                       return {\"cold_start_latency_ms\": elapsed_ms}\n",
    });

    let result = client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "cold_start_benchmark",
                "kind": "http",
                "spec": cold_start_spec,
            }),
        )
        .await;

    match result {
        Ok(ok) => {
            // An http-kind tool actually shipped -- acceptable per AC5.
            assert_eq!(common::extract_structured(&ok)["kind"], json!("http"));
        }
        Err(err) => {
            // A refusal is acceptable too, but only this specific,
            // actionable one -- never a generic invalid_spec that leaves the
            // caller guessing, and never (by construction, since publish
            // returned Err) a silent python-kind publish.
            assert_eq!(err.error_code.as_deref(), Some("kind_mismatch"));
            assert_eq!(err.data["requested"], json!("http"));
            assert_eq!(err.data["inferred"], json!("python"));
        }
    }

    // Either way, nothing published under `python` for this name.
    let list = client
        .tools_call("host.tool_list", json!({}))
        .await
        .expect("host.tool_list");
    let tools = common::extract_structured(&list)["tools"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        tools.iter().all(|t| t["kind"] != json!("python")),
        "this spec must never ship as python-kind: {tools:?}"
    );
}
