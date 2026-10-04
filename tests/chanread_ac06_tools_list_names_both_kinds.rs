//! PRD-mcphost-channel-read-name-parity
//! AC6 (P1) -- Given `tools/list`, When fetched, Then `host.channel.open`'s
//! description names both channel kinds and says both are readable.

use crate::common;
use common::{McpClient, TestServer, signup};

#[tokio::test]
async fn host_channel_open_description_names_both_kinds_as_readable() {
    let server = TestServer::start_with_signup_rate_limit(20).await;
    let (_ns, key) = signup(&server.base_url, "ChanRead AC6").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let listed = client.tools_list().await.expect("tools/list");
    let tools = listed["tools"].as_array().expect("tools array");
    let open_tool = tools
        .iter()
        .find(|t| t["name"].as_str() == Some("host.channel.open"))
        .expect("host.channel.open must be listed");
    let description = open_tool["description"].as_str().expect("description");

    assert!(description.contains("named"), "must name the named kind: {description}");
    assert!(description.contains("group"), "must name the group kind: {description}");
    assert!(
        description.to_lowercase().contains("readable"),
        "must say both kinds are readable: {description}"
    );
}
