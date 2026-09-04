//! PRD-mcphost-sandbox-ready
//! AC8 (P1) -- Given `sandbox_ready: false`, When a client reads
//! `host.get_info` instructions and the python kind's served description
//! (in `host.tool_publish`'s description), Then both state that
//! python-kind publishes are currently rejected with `sandbox_unavailable`
//! on this host.

mod common;
use common::{fake_interpreter_failing, python_kind_registry_with_selftest, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn get_info_and_tool_publish_description_both_note_the_unready_sandbox() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("skipped: no user namespaces (CI)");
        return;
    }
    let data_dir = common::TempDataDir::new();
    let (kinds, py) = python_kind_registry_with_selftest(&data_dir.0, 300);
    py.set_selftest_interpreter_for_test(Some(&fake_interpreter_failing(
        "bwrap: loopback: Failed RTM_NEWADDR: Operation not permitted",
    )));
    py.run_startup_selftest().await;

    let server = common::TestServer::start_with_kinds(kinds).await;
    let client = common::McpClient::new(&server.base_url);

    let init = client.initialize().await;
    let instructions = init["result"]["instructions"]
        .as_str()
        .expect("initialize result carries instructions");
    assert!(
        instructions.contains("sandbox_unavailable"),
        "host.get_info instructions must name sandbox_unavailable when unready: {instructions}"
    );

    let (_ns, key) = signup(&server.base_url, "AC8 Tenant").await;
    let tenant_client = common::McpClient::with_bearer(&server.base_url, &key);
    let listed = tenant_client.tools_list().await.expect("tools/list");
    let publish = listed["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|t| t["name"] == json!("host.tool_publish"))
        .expect("host.tool_publish is listed");
    let description = publish["description"]
        .as_str()
        .expect("description is a string");
    assert!(
        description.contains("sandbox_unavailable"),
        "host.tool_publish's description must name sandbox_unavailable when unready: {description}"
    );
}
