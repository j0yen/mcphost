//! PRD-mcphost-resume-tool AC2 -- Given a tenant with 3 tools, one called
//! successfully and one that failed with `tool_exception`, When `host.resume`
//! is called, Then `tools` has 3 entries, the two `last_call` fields match
//! the newest `calls` rows and the third is `null`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, publish, signup};
use mcphost::state::rfc3339_from_unix;
use serde_json::{Value, json};

fn entry<'a>(tools: &'a [Value], name: &str) -> &'a Value {
    tools.iter().find(|t| t["name"] == name).unwrap_or_else(|| panic!("{name} missing from {tools:?}"))
}

#[tokio::test]
async fn each_tool_carries_its_newest_calls_row_and_an_uncalled_tool_carries_null() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "RSM AC2").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let schema = json!({"type": "object"});
    for name in ["alpha", "beta", "gamma"] {
        publish(&client, name, "echo", json!({"schema": schema})).await;
    }

    // alpha: a real successful call.
    client.tools_call("host.tool.call", json!({"name": "alpha", "args": {}})).await.expect("alpha call");

    let conn = rusqlite::Connection::open(server.state.db.data_dir().join("mcphost.db")).unwrap();
    let tenant_id: i64 = conn.query_row("SELECT id FROM tenants WHERE namespace = ?1", [&ns], |r| r.get(0)).unwrap();
    // beta: an older success, then the newest row is a failure -- the newest wins.
    for (started, ok, class) in [(1_760_000_000_i64, 1, None), (1_760_000_500_i64, 0, Some("tool_exception"))] {
        conn.execute(
            "INSERT INTO calls (tenant_id, tool_name, started_at, started_unix, duration_ms, ok, error_class) \
             VALUES (?1, 'beta', 'unix:1.0', ?2, 4, ?3, ?4)",
            rusqlite::params![tenant_id, started, ok, class],
        )
        .unwrap();
    }

    let newest = |tool: &str| -> (i64, bool, Option<String>) {
        conn.query_row(
            "SELECT started_unix, ok, error_class FROM calls WHERE tenant_id = ?1 AND tool_name = ?2 \
             ORDER BY started_unix DESC, id DESC LIMIT 1",
            rusqlite::params![tenant_id, tool],
            |r| Ok((r.get(0)?, r.get::<_, i64>(1)? != 0, r.get(2)?)),
        )
        .unwrap()
    };

    let out = extract_structured(&client.tools_call("host.resume", json!({})).await.expect("host.resume"));
    let tools = out["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), 3, "{out}");

    for name in ["alpha", "beta"] {
        let (at, ok, class) = newest(name);
        let last = &entry(tools, name)["last_call"];
        assert_eq!(last["at"], rfc3339_from_unix(at).as_str(), "{name}: {last}");
        assert_eq!(last["ok"], ok, "{name}: {last}");
        assert_eq!(last["error_class"], json!(class), "{name}: {last}");
    }
    assert_eq!(entry(tools, "alpha")["last_call"]["ok"], true);
    assert_eq!(entry(tools, "beta")["last_call"]["ok"], false);
    assert_eq!(entry(tools, "beta")["last_call"]["error_class"], "tool_exception");
    assert!(entry(tools, "gamma")["last_call"].is_null(), "an uncalled tool is null");
}
