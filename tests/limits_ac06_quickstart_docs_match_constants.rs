//! AC6 (PRD-mcphost-call-limits-honest) — Given `host.quickstart`, When
//! read, Then `limits` contains the six new keys and their values equal the
//! constants; a test compares README and `llms.txt` numbers to the same
//! constants.

use crate::common;
use common::{TestServer, signup};
use mcphost::kinds::python::{DEFAULT_MAX_CONCURRENT_CALLS, MAX_TIMEOUT_S};
use mcphost::plans::PlanCatalog;
use mcphost::state::{CALL_TIMEOUT, MAX_REQUEST_BODY_BYTES, MAX_TOOL_OUTPUT_BYTES};
use serde_json::json;

#[tokio::test]
async fn quickstart_limits_object_matches_the_constants() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC6 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let result = client
        .tools_call("host.quickstart", json!({"kind": "echo"}))
        .await
        .expect("host.quickstart ok");
    let structured = common::extract_structured(&result);
    let limits = &structured["limits"];

    assert_eq!(
        limits["call_timeout_default_s"],
        json!(CALL_TIMEOUT.as_secs())
    );
    assert_eq!(limits["call_timeout_max_s"], json!(MAX_TIMEOUT_S));
    assert_eq!(limits["output_bytes_max"], json!(MAX_TOOL_OUTPUT_BYTES));
    assert_eq!(
        limits["request_body_bytes_max"],
        json!(MAX_REQUEST_BODY_BYTES)
    );
    assert_eq!(
        limits["concurrent_calls_host"],
        json!(DEFAULT_MAX_CONCURRENT_CALLS)
    );
    // A fresh signup is on the `free` plan.
    let catalog = PlanCatalog::default_catalog();
    let free = catalog.get("free").expect("free plan in default catalog");
    assert_eq!(
        limits["concurrent_calls_per_tenant"],
        json!(free.concurrent_calls_per_tenant)
    );
}

/// Requirement 5: README and `llms.txt` state the same numbers the code
/// enforces. Rather than a full markdown parser, this greps both files for
/// each constant's value rendered the same way the "Call limits"/"Limits
/// and pricing" sections were written -- a drift in either file (or in the
/// constants themselves, since the expected strings are built from the
/// constants, not hardcoded) fails this test.
#[test]
fn readme_and_llms_txt_state_the_same_numbers_as_the_constants() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let readme = std::fs::read_to_string(format!("{manifest_dir}/README.md"))
        .expect("read README.md");
    let llms = std::fs::read_to_string(format!("{manifest_dir}/www/llms.txt"))
        .expect("read www/llms.txt");

    let catalog = PlanCatalog::default_catalog();
    let free = catalog.get("free").expect("free plan");
    let pro = catalog.get("pro").expect("pro plan");
    let output_mib = MAX_TOOL_OUTPUT_BYTES / (1024 * 1024);
    let body_mib = MAX_REQUEST_BODY_BYTES / (1024 * 1024);

    let expectations: Vec<String> = vec![
        format!("{} s by default", CALL_TIMEOUT.as_secs()),
        format!("{MAX_TIMEOUT_S} s max"),
        format!("{output_mib} MiB"),
        format!("{body_mib} MiB"),
        format!("{DEFAULT_MAX_CONCURRENT_CALLS} concurrent calls host-wide"),
        format!(
            "{} per tenant on the free plan",
            free.concurrent_calls_per_tenant
        ),
        format!("{} on pro", pro.concurrent_calls_per_tenant),
    ];

    for expectation in &expectations {
        assert!(
            readme.contains(expectation.as_str()),
            "README.md must state {expectation:?} (derived from the live constants)"
        );
        assert!(
            llms.contains(expectation.as_str()),
            "www/llms.txt must state {expectation:?} (derived from the live constants)"
        );
    }
}
