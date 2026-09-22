//! PRD-mcphost-team-memory
//! AC7 — Given the synthorg task, When run 5 times, Then 5
//! first-cross-agent-recall times are recorded.
//!
//! Same shape as `tests/mcphost_share_a_tool_not_a_key_ac07_synthorg_task_five_completions.rs`:
//! RedBaron's actual corpus repository lives outside this worktree's scope
//! (`build_into: mcphost` only), so this test loads
//! `examples/team-memory/synthorg-task.yaml` at test time, drives its
//! steps against a real in-process server as an mcphost-recognized
//! synthorg session, and scores each run's completion with the task's own
//! `gold.call_check` expression parsed straight out of the YAML.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, extract_structured, poll_until_ready, python_kind_registry};
use mcphost::sandbox;
use mcphost::state::is_known_synthorg_client;
use serde_json::{Value, json};
use std::time::Duration;

const SYNTHORG_CLIENT_NAME: &str = "synthorg-mcphost-corpus";

struct Gold {
    tool_name: String,
    kind: String,
    call_check: String,
}

/// Hand-rolled rather than pulling in a YAML crate for one small,
/// self-owned fixture: reads the `gold:` block's three scalar fields out
/// of `examples/team-memory/synthorg-task.yaml` directly.
fn load_gold() -> Gold {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/team-memory/synthorg-task.yaml");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));

    let mut tool_name = None;
    let mut kind = None;
    let mut call_check = None;
    let mut in_gold = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == "gold:" {
            in_gold = true;
            continue;
        }
        if !in_gold {
            continue;
        }
        if let Some(v) = trimmed.strip_prefix("tool_name:") {
            tool_name = Some(v.trim().trim_matches('"').to_string());
        } else if let Some(v) = trimmed.strip_prefix("call_check:") {
            call_check = Some(v.trim().trim_matches('"').to_string());
        } else if let Some(v) = trimmed.strip_prefix("kind:") {
            kind = Some(v.trim().trim_matches('"').to_string());
        }
    }
    Gold {
        tool_name: tool_name.expect("synthorg-task.yaml: gold.tool_name"),
        kind: kind.expect("synthorg-task.yaml: gold.kind"),
        call_check: call_check.expect("synthorg-task.yaml: gold.call_check"),
    }
}

/// RedBaron's mini-grammar is `<dotted.path rooted at "result"> == <literal>`.
fn eval_call_check(call_check: &str, result: &Value) -> bool {
    let (path, rhs) = call_check
        .split_once("==")
        .unwrap_or_else(|| panic!("call_check is not `path == literal`: {call_check}"));
    let rhs = rhs.trim();
    let expected = if rhs == "true" {
        Value::Bool(true)
    } else if rhs == "false" {
        Value::Bool(false)
    } else if let Ok(n) = rhs.parse::<f64>() {
        json!(n)
    } else {
        Value::String(rhs.trim_matches('"').to_string())
    };

    let mut segments = path.trim().split('.');
    assert_eq!(
        segments.next(),
        Some("result"),
        "call_check path must be rooted at `result.`: {call_check}"
    );
    let mut cur = result;
    for seg in segments {
        match cur.get(seg) {
            Some(v) => cur = v,
            None => return false,
        }
    }
    *cur == expected
}

struct Completion {
    wall_time_ms: u64,
}

/// Runs the task's own recipe end to end -- table, publish `remember`
/// and `recall`, group, share both, add the second tenant, first tenant
/// remembers, second recalls -- then scores completion with
/// `gold.call_check` against the second tenant's real structured result.
async fn run_task_once(gold: &Gold, envs_dir: &TempDataDir) -> Completion {
    let start = std::time::Instant::now();

    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;

    let owner_signup_client =
        McpClient::new(&server.base_url).with_client_info(SYNTHORG_CLIENT_NAME, "1.0.0");
    let owner_signup = owner_signup_client
        .tools_call("signup", json!({"name": "team-memory-owner"}))
        .await
        .expect("owner signup");
    let owner_struct = extract_structured(&owner_signup);
    let owner_ns = owner_struct["tenant"].as_str().expect("owner tenant").to_string();
    let owner_key = owner_struct["key"].as_str().expect("owner key").to_string();
    let owner = McpClient::with_bearer(&server.base_url, &owner_key)
        .with_client_info(SYNTHORG_CLIENT_NAME, "1.0.0");

    let caller_signup_client =
        McpClient::new(&server.base_url).with_client_info(SYNTHORG_CLIENT_NAME, "1.0.0");
    let caller_signup = caller_signup_client
        .tools_call("signup", json!({"name": "team-memory-caller"}))
        .await
        .expect("caller signup");
    let caller_struct = extract_structured(&caller_signup);
    let caller_ns = caller_struct["tenant"].as_str().expect("caller tenant").to_string();
    let caller_key = caller_struct["key"].as_str().expect("caller key").to_string();
    let caller = McpClient::with_bearer(&server.base_url, &caller_key)
        .with_client_info(SYNTHORG_CLIENT_NAME, "1.0.0");

    owner
        .tools_call(
            "host.table.create",
            json!({
                "name": "memory",
                "columns": {"key": "text", "text": "text", "tags": "json", "writer": "text", "at": "real"},
            }),
        )
        .await
        .expect("table create");

    let remember_source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/team-memory/tools/remember.py"),
    )
    .expect("read remember.py");
    owner
        .tools_call(
            "host.tool_publish",
            json!({"name": "remember", "kind": gold.kind, "spec": {"source": remember_source}}),
        )
        .await
        .expect("publish remember");

    let recall_source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/team-memory/tools/recall.py"),
    )
    .expect("read recall.py");
    owner
        .tools_call(
            "host.tool_publish",
            json!({"name": gold.tool_name, "kind": gold.kind, "spec": {"source": recall_source}}),
        )
        .await
        .expect("publish recall");

    owner
        .tools_call("host.group.create", json!({"name": "team"}))
        .await
        .expect("group create");
    owner
        .tools_call(
            "host.tool_share",
            json!({"name": "remember", "visibility": "group", "group": "team"}),
        )
        .await
        .expect("share remember");
    owner
        .tools_call(
            "host.tool_share",
            json!({"name": gold.tool_name, "visibility": "group", "group": "team"}),
        )
        .await
        .expect("share recall");
    owner
        .tools_call("host.group.add", json!({"name": "team", "namespace": caller_ns}))
        .await
        .expect("group add");

    poll_until_ready(
        &owner,
        &format!("{owner_ns}.remember"),
        json!({"key": "deploy", "text": "deploy window is Tuesday", "who": owner_ns}),
        Duration::from_secs(10),
    )
    .await
    .unwrap_or_else(|e| panic!("owner remember must succeed: {} {}", e.code, e.message));

    let recall_qualified = format!("{owner_ns}.{}", gold.tool_name);
    let caller_call = poll_until_ready(&caller, &recall_qualified, json!({"query": "deploy"}), Duration::from_secs(10))
        .await
        .unwrap_or_else(|e| panic!("caller recall must succeed: {} {}", e.code, e.message));
    let structured = extract_structured(&caller_call);

    assert!(
        eval_call_check(&gold.call_check, &structured),
        "task's own gold.call_check `{}` did not pass against the caller's result: {structured}",
        gold.call_check
    );

    Completion {
        wall_time_ms: start.elapsed().as_millis() as u64,
    }
}

#[tokio::test]
async fn synthorg_task_runs_five_times_with_five_completions_and_wall_time() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    assert!(
        is_known_synthorg_client(SYNTHORG_CLIENT_NAME),
        "this test's own client name must be one mcphost's synthorg classifier recognizes, \
         or it isn't actually exercising a synthorg-shaped session"
    );

    let gold = load_gold();
    assert_eq!(gold.tool_name, "recall");
    assert_eq!(gold.kind, "python");

    let envs_dir = TempDataDir::new();
    let mut completions = Vec::new();
    for _ in 0..5 {
        completions.push(run_task_once(&gold, &envs_dir).await);
    }

    assert_eq!(completions.len(), 5, "expected 5 completions from 5 runs of the synthorg task");
    for completion in &completions {
        assert!(completion.wall_time_ms > 0, "each completion must record a wall time");
    }
}
