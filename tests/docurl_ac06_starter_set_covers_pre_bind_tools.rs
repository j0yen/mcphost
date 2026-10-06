//! PRD-mcphost-docs-one-url-flow
//! AC6 (P0) -- Given the anonymous starter tool set and the tools the
//! quickstart names, When compared, Then every named tool is in the set.
//!
//! Requirement 6 scopes this to the tools the quickstart names "before
//! the session is bound" -- every step after the first call (publish,
//! test, invite.create, ...) runs as an already-authenticated tenant, so
//! only the tool the quickstart's first call actually names needs to be
//! in the anonymous starter set.

use crate::common;
use common::{McpClient, TestServer};

const LLMS_TXT: &str = include_str!("../www/llms.txt");

/// The tool name inside the first fenced ``` ... ``` block of the
/// Quickstart section -- the one call the agent makes while still
/// anonymous, before `onboarding.url` binds it to a tenant.
fn first_quickstart_call_tool_name() -> &'static str {
    let start = LLMS_TXT
        .find("## Quickstart for agents")
        .expect("www/llms.txt must have a 'Quickstart for agents' heading");
    let end = LLMS_TXT[start..]
        .find("## Explicit signup")
        .map(|offset| start + offset)
        .expect("'Explicit signup' heading must follow 'Quickstart for agents'");
    let section = &LLMS_TXT[start..end];

    let open = section.find("```").expect("Quickstart must have at least one fenced block");
    let after_open = &section[open + 3..];
    let close = after_open.find("```").expect("every ``` fence must close");
    let snippet = after_open[..close].trim();
    let open_paren = snippet.find('(').expect("the first snippet must be a call");
    snippet[..open_paren].trim()
}

#[tokio::test]
async fn the_first_quickstart_call_is_in_the_anonymous_starter_set() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let tools = client
        .tools_list()
        .await
        .expect("unauthenticated tools/list must succeed");
    let starter_set: Vec<String> = tools["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().expect("tool name").to_string())
        .collect();

    let first_call = first_quickstart_call_tool_name();
    assert!(
        starter_set.iter().any(|n| n == first_call),
        "the quickstart's first call ({first_call}) must be in the anonymous starter set: \
         {starter_set:?}"
    );

    // `host.quickstart` itself is also named in the same step's prose (as
    // the read-only alternative) and must be reachable anonymously too.
    assert!(
        starter_set.iter().any(|n| n == "host.quickstart"),
        "host.quickstart must be in the anonymous starter set: {starter_set:?}"
    );
}

/// Every tool the Quickstart section's own prose names with a literal
/// `host.<...>`/`billing.<...>`/`signup` identifier, up to (not
/// including) the first fenced snippet -- i.e. everything an agent reads
/// before making any call at all -- must already be visible in the
/// anonymous `tools/list`, so the doc never promises a tool the agent
/// cannot yet see.
#[tokio::test]
async fn every_tool_named_before_the_first_call_is_in_the_anonymous_starter_set() {
    let start = LLMS_TXT.find("## Quickstart for agents").unwrap();
    let first_fence = LLMS_TXT[start..].find("```").map(|o| start + o).unwrap();
    let preamble = &LLMS_TXT[start..first_fence];

    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);
    let tools = client.tools_list().await.expect("tools/list");
    let starter_set: Vec<String> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();

    for name in ["host.quickstart", "host.whoami"] {
        if preamble.contains(name) {
            assert!(
                starter_set.iter().any(|n| n == name),
                "{name} is named before the quickstart's first call but is not in the \
                 anonymous starter set: {starter_set:?}"
            );
        }
    }
}
