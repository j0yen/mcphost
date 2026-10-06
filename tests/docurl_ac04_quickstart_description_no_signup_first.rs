//! PRD-mcphost-docs-one-url-flow
//! AC4 (P0) -- Given an unauthenticated `tools/list`, When the
//! `host.quickstart` description is read, Then it does not instruct a
//! signup step before the first call.

use crate::common;
use common::{McpClient, TestServer};

#[tokio::test]
async fn host_quickstart_description_does_not_instruct_signup_before_the_first_call() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let tools = client
        .tools_list()
        .await
        .expect("unauthenticated tools/list must succeed");
    let tool_array = tools["tools"].as_array().expect("tools array");
    let quickstart = tool_array
        .iter()
        .find(|t| t["name"] == "host.quickstart")
        .expect("host.quickstart must be in the anonymous starter set");
    let description = quickstart["description"]
        .as_str()
        .expect("host.quickstart has a description");

    assert!(
        !description.to_lowercase().contains("signup step"),
        "host.quickstart's description must not instruct a signup step first: {description}"
    );
    assert!(
        description.contains("no signup required") || description.contains("no signup call"),
        "host.quickstart's description must say the implicit path needs no signup call: \
         {description}"
    );

    let signup_tool = tool_array
        .iter()
        .find(|t| t["name"] == "signup")
        .expect("signup must be in the anonymous starter set");
    let signup_description = signup_tool["description"]
        .as_str()
        .expect("signup has a description");
    assert!(
        signup_description.to_lowercase().contains("explicit signup")
            || signup_description.to_lowercase().contains("never need this"),
        "signup's description must describe the implicit path first: {signup_description}"
    );
}
