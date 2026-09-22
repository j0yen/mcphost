//! PRD-mcphost-uptime-probes
//! AC7 — Given the synthorg task, When run 5 times, Then 5 completions
//! record time to first stored check.
//!
//! Same shape as
//! `tests/mcphost_team_memory_ac07_synthorg_task_five_completions.rs`:
//! RedBaron's actual corpus repository lives outside this worktree's scope
//! (`build_into: mcphost` only), so this test loads
//! `examples/uptime-probes/synthorg-task.yaml` at test time, drives its
//! steps against a real in-process server as an mcphost-recognized
//! synthorg session, and scores each run's completion with the task's own
//! `gold.call_check` expression parsed straight out of the YAML.

use crate::common;
use crate::uptime_probes;

use common::{McpClient, TempDataDir, TestServer, extract_structured, poll_until_ready, python_kind_registry};
use mcphost::sandbox;
use mcphost::state::is_known_synthorg_client;
use serde_json::{Value, json};
use std::time::Duration;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const SYNTHORG_CLIENT_NAME: &str = "synthorg-mcphost-corpus";

struct Gold {
    tool_name: String,
    kind: String,
    call_check: String,
}

/// Hand-rolled rather than pulling in a YAML crate for one small,
/// self-owned fixture: reads the `gold:` block's three scalar fields out
/// of `examples/uptime-probes/synthorg-task.yaml` directly.
fn load_gold() -> Gold {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/uptime-probes/synthorg-task.yaml");
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
    // Compare numbers by value (`probed == 1` must match a JSON integer
    // `1` even though `expected` was parsed as an f64 above -- serde_json's
    // `Number` doesn't treat `1` and `1.0` as `==` since they carry
    // different internal representations).
    match (cur.as_f64(), expected.as_f64()) {
        (Some(a), Some(b)) => a == b,
        _ => *cur == expected,
    }
}

struct Completion {
    wall_time_ms: u64,
}

/// Runs the task's own recipe end to end -- tables, one target, publish
/// `probe`, call it -- then scores completion with `gold.call_check`
/// against the real structured result ("time to first stored check").
async fn run_task_once(gold: &Gold, envs_dir: &TempDataDir, upstream_url: &str) -> Completion {
    let start = std::time::Instant::now();

    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;

    let signup_client = McpClient::new(&server.base_url).with_client_info(SYNTHORG_CLIENT_NAME, "1.0.0");
    let signup_resp = signup_client
        .tools_call("signup", json!({"name": "uptime-probes-owner"}))
        .await
        .expect("signup");
    let signup_struct = extract_structured(&signup_resp);
    let ns = signup_struct["tenant"].as_str().expect("tenant").to_string();
    let key = signup_struct["key"].as_str().expect("key").to_string();
    let client = McpClient::with_bearer(&server.base_url, &key).with_client_info(SYNTHORG_CLIENT_NAME, "1.0.0");

    uptime_probes::create_tables(&client).await;
    client
        .tools_call(
            "host.state.insert",
            json!({"table": "targets", "rows": [{"url": upstream_url, "added_at": 1.0}]}),
        )
        .await
        .expect("insert target");

    let probe_source = uptime_probes::probe_source();
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": gold.tool_name, "kind": gold.kind, "spec": {"source": probe_source, "network": "public"}}),
        )
        .await
        .expect("publish probe");

    let qualified = format!("{ns}.{}", gold.tool_name);
    let call = poll_until_ready(&client, &qualified, json!({}), Duration::from_secs(20))
        .await
        .unwrap_or_else(|e| panic!("probe call must succeed: {} {}", e.code, e.message));
    let structured = extract_structured(&call);

    assert!(
        eval_call_check(&gold.call_check, &structured),
        "task's own gold.call_check `{}` did not pass against the result: {structured}",
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
    assert_eq!(gold.tool_name, "probe");
    assert_eq!(gold.kind, "python");

    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&upstream)
        .await;
    let upstream_url = format!("{}/health", upstream.uri());

    let envs_dir = TempDataDir::new();
    let mut completions = Vec::new();
    for _ in 0..5 {
        completions.push(run_task_once(&gold, &envs_dir, &upstream_url).await);
    }

    assert_eq!(completions.len(), 5, "expected 5 completions from 5 runs of the synthorg task");
    for completion in &completions {
        assert!(completion.wall_time_ms > 0, "each completion must record a wall time");
    }
}
