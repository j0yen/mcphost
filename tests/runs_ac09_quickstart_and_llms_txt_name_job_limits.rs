//! AC9 (P0) — Given `host.quickstart` and `llms.txt`, When read, Then both
//! name `host.runs.get`, `host.runs.list`, `host.runs.cancel` and the job
//! limits.

mod common;
use common::{TestServer, extract_structured, signup};
use serde_json::json;

const LLMS_TXT: &str = include_str!("../www/llms.txt");

#[tokio::test]
async fn quickstart_names_job_limits() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC9 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let quickstart = extract_structured(
        &client
            .tools_call("host.quickstart", json!({"kind": "echo"}))
            .await
            .expect("quickstart ok"),
    );
    let plan_limits = &quickstart["limits"]["plan"];
    assert_eq!(plan_limits["job_max_s"], json!(300), "quickstart: {quickstart}");
    assert_eq!(plan_limits["jobs_concurrent"], json!(1), "quickstart: {quickstart}");
}

#[test]
fn llms_txt_names_the_runs_tools_and_job_limits() {
    for tool in ["host.runs.get", "host.runs.list", "host.runs.cancel"] {
        assert!(
            LLMS_TXT.contains(&format!("`{tool}`")),
            "llms.txt must name {tool}"
        );
    }
    assert!(
        LLMS_TXT.contains("job_max_s") && LLMS_TXT.contains("jobs_concurrent"),
        "llms.txt must name the job limits"
    );
}
