//! PRD-mcphost-tool-reach-manifest-v2
//! AC3 (P0) -- Given a python tool calling `mcphost.state.set(key, v)` with a
//! non-literal key and another that reaches `mcphost.state` through an alias,
//! When read, Then the first has `"*dynamic*"` in `state.writes` and nothing
//! else from that call, and the second carries `"*dynamic*"` in `reads`,
//! `writes` and `tables`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::{Value, json};

const DYNAMIC: &str = "*dynamic*";

async fn reach_of(client: &McpClient, name: &str, source: &str) -> Value {
    client
        .tools_call("host.tool.publish", json!({"name": name, "kind": "python", "spec": {"source": source}}))
        .await
        .unwrap_or_else(|e| panic!("publish {name}: {} {}", e.code, e.message));
    let result = client
        .tools_call("host.tool.reach", json!({"name": name}))
        .await
        .unwrap_or_else(|e| panic!("host.tool.reach {name}: {} {}", e.code, e.message));
    extract_structured(&result)["reach"]["state"].clone()
}

#[tokio::test]
async fn trm_ac03_a_non_literal_key_and_an_alias_read_as_dynamic() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs.0)).await;
    let (_ns, key) = signup(&server.base_url, "TRM AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // A non-literal key: dynamic in writes, and nothing else from that call.
    let state = reach_of(
        &client,
        "dyn_key",
        "import mcphost\n\ndef main(args):\n    mcphost.state.set(args[\"key\"], 1)\n    return {}\n",
    )
    .await;
    assert_eq!(state, json!({"reads": [], "writes": [DYNAMIC], "tables": []}));

    // A key built by concatenation is not a literal either.
    let state = reach_of(
        &client,
        "dyn_concat",
        "import mcphost\n\ndef main(args):\n    return mcphost.state.get(\"orders/\" + args[\"id\"])\n",
    )
    .await;
    assert_eq!(state, json!({"reads": [DYNAMIC], "writes": [], "tables": []}));

    // An alias reaches every list, and the literal call beside it stays literal.
    let everywhere = json!({"reads": [DYNAMIC], "writes": [DYNAMIC], "tables": [DYNAMIC]});
    let state = reach_of(
        &client,
        "alias_assign",
        "import mcphost\n\ndef main(args):\n    s = mcphost.state\n    s.set(\"a\", 1)\n    return {}\n",
    )
    .await;
    assert_eq!(state, everywhere);

    let state = reach_of(
        &client,
        "alias_from_import",
        "from mcphost import state\n\ndef main(args):\n    return state.get(\"a\")\n",
    )
    .await;
    assert_eq!(state, everywhere);

    let state = reach_of(
        &client,
        "alias_getattr",
        "import mcphost\n\ndef main(args):\n    return getattr(mcphost, \"state\").get(\"a\")\n",
    )
    .await;
    assert_eq!(state, everywhere);

    // The literal spellings stay exact: keyword form, a call split over lines,
    // and the table verbs.
    let state = reach_of(
        &client,
        "literals",
        "import mcphost\n\ndef main(args):\n    mcphost.state.get(key=\"cfg/a\")\n    mcphost.state.set(\n        'out/b', 1)\n    mcphost.state.query('events')\n    mcphost.state.insert(\"events\", [])\n    return {}\n",
    )
    .await;
    assert_eq!(state, json!({"reads": ["cfg/a"], "writes": ["out/b"], "tables": ["events"]}));
}
