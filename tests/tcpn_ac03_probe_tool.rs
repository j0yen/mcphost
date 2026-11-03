//! PRD-mcphost-tools-call-progress-notifications
//! AC3 (P0) — Given `host.probe.progress` registered through the generated
//! dispatch table, When it is called with `{steps: 3, step_ms: 1000}` under a
//! test `ProgressSink` that records reports, Then the sink receives exactly
//! `(33, "step 1/3")`, `(66, "step 2/3")`, `(100, "step 3/3")` in order with
//! at least 1000 ms between them, the result is `{steps: 3, elapsed_ms: >= 3000}`,
//! `{steps: 6, step_ms: 1000}`, `{steps: 3, step_ms: 500}` and `{steps: 3}`
//! return the standard invalid-arguments error, and the tool appears in
//! `tools/list` with that schema.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::kinds::ProgressSink;
use serde_json::json;
use std::sync::Mutex;
use std::time::Instant;

type Report = (Option<i64>, Option<String>, Instant);

#[derive(Default)]
struct Recorder(Mutex<Vec<Report>>);

impl ProgressSink for Recorder {
    fn report(&self, pct: Option<i64>, msg: Option<String>) {
        self.0.lock().expect("recorder").push((pct, msg, Instant::now()));
    }
}

#[tokio::test]
async fn tcpn_ac03_probe_reports_on_schedule() {
    assert!(
        mcphost::dispatch_table::DISPATCH_TABLE.iter().any(|(verb, _)| *verb == "host.probe.progress"),
        "host.probe.progress is a row of the generated dispatch table"
    );
    let sink = Recorder::default();
    let result = mcphost::progress::probe(&json!({"steps": 3, "step_ms": 1000}), &sink).await.expect("probe");
    assert_eq!(result["steps"], json!(3));
    assert!(result["elapsed_ms"].as_i64().expect("elapsed_ms") >= 3000, "{result}");
    let reports = sink.0.lock().expect("recorder");
    let got: Vec<_> = reports.iter().map(|(p, m, _)| (*p, m.clone())).collect();
    assert_eq!(
        got,
        vec![
            (Some(33), Some("step 1/3".to_string())),
            (Some(66), Some("step 2/3".to_string())),
            (Some(100), Some("step 3/3".to_string())),
        ]
    );
    for pair in reports.windows(2) {
        let gap_ms = pair[1].2.duration_since(pair[0].2).as_millis();
        assert!(gap_ms >= 1000, "reports are >= 1000 ms apart, got {gap_ms}");
    }
}

#[tokio::test]
async fn tcpn_ac03_probe_rejects_out_of_range_arguments() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "tcpn probe").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    for args in [json!({"steps": 6, "step_ms": 1000}), json!({"steps": 3, "step_ms": 500}), json!({"steps": 3})] {
        let err = client.tools_call("host.probe.progress", args.clone()).await.expect_err(&format!("{args} must be refused"));
        assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{args}: {}", err.message);
    }

    let list = client.tools_list().await.expect("tools/list");
    let tool = list["tools"].as_array().expect("tools").iter().find(|t| t["name"] == "host.probe.progress").expect("host.probe.progress listed");
    let props = &tool["inputSchema"]["properties"];
    assert_eq!(props["steps"]["minimum"], json!(1));
    assert_eq!(props["steps"]["maximum"], json!(5));
    assert_eq!(props["step_ms"]["minimum"], json!(1000));
    assert_eq!(props["step_ms"]["maximum"], json!(5000));
    assert_eq!(tool["inputSchema"]["required"], json!(["steps", "step_ms"]));
}
