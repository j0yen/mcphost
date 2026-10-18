//! PRD-mcphost-runs-list-filter-enums-generated
//! AC2 (P0) -- Given the same server, When `host.runs.list` is called with
//! `trigger: "not-a-trigger"` and, separately, `verdict: "maybe"`, Then each
//! is refused as in AC 1 with `path` naming its own field and `expected` the
//! matching `names()`, and a counter in `runs::list` proves the handler body
//! never ran for any of the three refused calls.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::budget::Verdict;
use mcphost::host_args::handler_runs;
use mcphost::runs::{RunStatus, RunTrigger};
use serde_json::json;

const BODY: &str = "host.runs.list#body";

#[tokio::test]
async fn rlfe_ac02() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "RLFE AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // The counter moves for a call the validator lets through.
    let before = handler_runs(&ns, BODY);
    client.tools_call("host.runs.list", json!({"status": "done"})).await.expect("valid call");
    assert_eq!(handler_runs(&ns, BODY), before + 1, "the counter must see a valid call");

    let before = handler_runs(&ns, BODY);
    let cases = [
        ("status", json!("not-a-status"), json!(RunStatus::names())),
        ("trigger", json!("not-a-trigger"), json!(RunTrigger::names())),
        ("verdict", json!("maybe"), json!(Verdict::names())),
    ];
    for (field, value, expected) in cases {
        let err = client
            .tools_call("host.runs.list", json!({ field: value }))
            .await
            .expect_err("an out-of-set filter must be refused");
        assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{field}: {err:?}");
        assert!(err.data.is_object(), "{field}: data must be an object: {:?}", err.data);
        assert_eq!(err.data["path"], json!(format!("/{field}")), "{:?}", err.data);
        assert_eq!(err.data["constraint"], json!("enum"), "{:?}", err.data);
        assert_eq!(err.data["expected"], expected, "{field}: {:?}", err.data);
    }
    assert_eq!(handler_runs(&ns, BODY), before, "runs::list ran for a refused call");
}
