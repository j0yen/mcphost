//! PRD-mcphost-tool-idempotent-declaration AC6 -- Given a tool called from
//! the sandbox, When it reads `mcphost.tool_meta()`, Then `idempotent`
//! matches the listing.
//!
//! Two halves:
//!
//! * `runner_serves_tool_meta_from_each_calls_payload` drives the real
//!   runner script (`py_runner_script()`, the file every sandbox runs) with
//!   plain `python3`, no user namespaces needed: `mcphost.tool_meta()`
//!   returns the `tool_meta` of the payload line of THAT call, and a second
//!   line on the same (warm) process replaces it.
//! * `sandboxed_tool_meta_matches_the_listing` is the end-to-end proof
//!   through `host.tool_publish` -> `tools/list` -> a real sandboxed call;
//!   it needs unprivileged user namespaces and follows the crate's
//!   skip-in-CI / fail-loudly-elsewhere convention.

use crate::common;
use common::{McpClient, TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::kinds::python::py_runner_script;
use mcphost::sandbox;
use serde_json::{Value, json};
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

const SOURCE: &str = "import mcphost\ndef main(args):\n    return {\"meta\": mcphost.tool_meta()}\n";

#[test]
fn runner_serves_tool_meta_from_each_calls_payload() {
    let dir = common::TempDataDir::new();
    std::fs::write(dir.0.join("runner.py"), py_runner_script()).unwrap();
    std::fs::write(dir.0.join("tool.py"), SOURCE).unwrap();
    let mut child = Command::new("python3")
        .arg("runner.py")
        .current_dir(&dir.0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn python3 runner");
    let mut stdin = child.stdin.take().unwrap();
    for payload in [
        json!({"args": {}, "tool_meta": {"idempotent": true}}),
        json!({"args": {}, "tool_meta": {"idempotent": false}}),
        json!({"args": {}}),
    ] {
        writeln!(stdin, "{payload}").unwrap();
    }
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    let lines: Vec<Value> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("runner line {l:?}: {e}")))
        .collect();
    assert_eq!(lines.len(), 3, "stdout: {} stderr: {}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let meta = |i: usize| lines[i].pointer("/result/meta").cloned().unwrap_or_else(|| panic!("line {i}: {}", lines[i]));
    assert_eq!(meta(0), json!({"idempotent": true}));
    assert_eq!(meta(1), json!({"idempotent": false}));
    assert_eq!(meta(2), json!({"idempotent": false}), "a payload without tool_meta falls back to the safe default");
}

#[tokio::test]
async fn sandboxed_tool_meta_matches_the_listing() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "IdeclAc06").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    for (name, extra, expected) in [("meta_plain", json!({}), false), ("meta_idem", json!({"idempotent": true}), true)] {
        let mut spec = json!({"source": SOURCE});
        spec.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        client
            .tools_call("host.tool_publish", json!({"name": name, "kind": "python", "spec": spec}))
            .await
            .unwrap_or_else(|e| panic!("publish {name}: {}", e.message));
        let qualified = format!("{ns}.{name}");
        let called = poll_until_ready(&client, &qualified, json!({}), Duration::from_secs(10))
            .await
            .unwrap_or_else(|e| panic!("sandboxed call {name}: {} {}", e.code, e.message));
        let from_sandbox = extract_structured(&called)["meta"]["idempotent"].clone();
        let listing = client.tools_list().await.expect("tools/list");
        let listed = listing["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == qualified)
            .expect("listed")["annotations"]["idempotentHint"]
            .clone();
        assert_eq!(from_sandbox, listed, "{name}: sandbox tool_meta vs listing");
        assert_eq!(from_sandbox, json!(expected), "{name}");
    }
}
