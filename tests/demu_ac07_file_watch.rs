//! PRD-mcphost-dev-emulator
//! AC7 (P1) -- Given a running emulator, When `tool.py` changes on disk,
//! Then within 1 s `tools/list` returns the new args schema and a
//! `notifications/tools/list_changed` is sent.

use crate::common::ToolDirFixture;
use futures_util::StreamExt;
use mcphost::devserver::{Dev, DevOpts, serve};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

const TOML: &str = "[tool]\nname = \"report\"\nkind = \"python\"\n";
const BEFORE: &str = "def main(args):\n    n = args[\"n\"]\n    return {\"n\": n}\n";
const AFTER: &str = "def main(args):\n    n = args[\"n\"]\n    label = args.get(\"label\", \"x\")\n    return {\"n\": n, \"label\": label}\n";

async fn list(addr: std::net::SocketAddr) -> Value {
    reqwest::Client::new()
        .post(format!("http://{addr}/mcp"))
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}))
        .send()
        .await
        .expect("post /mcp")
        .json()
        .await
        .expect("json body")
}

#[tokio::test]
async fn demu_ac07_a_changed_tool_py_relists_and_notifies_within_a_second() {
    let dir = ToolDirFixture::from_example("hello-py");
    dir.write("tool.toml", TOML);
    dir.write("tool.py", BEFORE);
    let dev = Dev::open(&DevOpts { dir: dir.0.clone(), no_sandbox: true, plan: "free".into() })
        .await
        .expect("open the directory");
    let (addr, task) = serve(Arc::new(dev), 0).await.expect("bind");

    let before = list(addr).await;
    assert!(before["result"]["tools"][0]["inputSchema"]["properties"]["label"].is_null(), "{before}");

    // The notification stream, opened before the edit.
    let stream = reqwest::Client::new()
        .get(format!("http://{addr}/mcp"))
        .header("accept", "text/event-stream")
        .send()
        .await
        .expect("open the event stream");
    let mut chunks = stream.bytes_stream();

    dir.write("tool.py", AFTER);

    let notified = tokio::time::timeout(Duration::from_secs(1), async {
        let mut seen = String::new();
        while let Some(chunk) = chunks.next().await {
            seen.push_str(&String::from_utf8_lossy(&chunk.expect("stream chunk")));
            if seen.contains("notifications/tools/list_changed") {
                return seen;
            }
        }
        seen
    })
    .await
    .expect("list_changed arrives within 1 s of the edit");
    assert!(notified.contains("notifications/tools/list_changed"), "{notified}");

    // The reload finishes before the notification is sent, so the very next
    // listing already carries the new schema.
    let after = list(addr).await;
    assert_eq!(
        after["result"]["tools"][0]["inputSchema"]["properties"]["label"]["type"],
        json!("string"),
        "{after}"
    );
    task.abort();
}
