//! PRD-mcphost-database-in-a-minute
//! AC7 — Given the synthorg task, When run 5 times, Then 5 exact-answer
//! completions are recorded.
//!
//! Same shape as `tests/mcphost_team_memory_ac07_synthorg_task_five_completions.rs`:
//! RedBaron's actual corpus repository lives outside this worktree's scope
//! (`build_into: mcphost` only), so this test loads
//! `examples/database-in-a-minute/synthorg-task.yaml` at test time, drives
//! its steps against a real in-process server as an mcphost-recognized
//! synthorg session, and scores each run's completion with the task's own
//! `gold.call_check` expression parsed straight out of the YAML.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, extract_structured, poll_until_ready, python_kind_registry};
use mcphost::sandbox;
use mcphost::state::is_known_synthorg_client;
use serde_json::{Value, json};
use std::time::Duration;

const SYNTHORG_CLIENT_NAME: &str = "synthorg-mcphost-corpus";
const FIXTURE_CSV: &str = include_str!("../examples/database-in-a-minute/fixture.csv");
const BATCH_SIZE: usize = 200;

struct Gold {
    tool_name: String,
    kind: String,
    call_check: String,
}

/// Hand-rolled rather than pulling in a YAML crate for one small,
/// self-owned fixture: reads the `gold:` block's three scalar fields out
/// of `examples/database-in-a-minute/synthorg-task.yaml` directly.
fn load_gold() -> Gold {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/database-in-a-minute/synthorg-task.yaml");
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
    // A whole-number literal parses as a JSON float above (`json!(n)` with
    // `n: f64`), but `count` on the wire is a JSON integer -- compare
    // numbers by value, not by serde_json::Number's int/float variant.
    match (cur.as_f64(), expected.as_f64()) {
        (Some(a), Some(b)) => a == b,
        _ => *cur == expected,
    }
}

/// Parses `fixture.csv` (no embedded commas or quotes -- id,category,amount,day)
/// into JSON row objects, no `csv` crate dependency needed for this one
/// self-owned fixture.
fn fixture_rows() -> Vec<Value> {
    let mut lines = FIXTURE_CSV.lines();
    lines.next(); // header
    lines
        .filter(|l| !l.is_empty())
        .map(|line| {
            let mut cols = line.split(',');
            let id: i64 = cols.next().unwrap().parse().unwrap();
            let category = cols.next().unwrap().to_string();
            let amount: f64 = cols.next().unwrap().parse().unwrap();
            let day: i64 = cols.next().unwrap().parse().unwrap();
            json!({"id": id, "category": category, "amount": amount, "day": day})
        })
        .collect()
}

struct Completion {
    wall_time_ms: u64,
}

/// Runs the task's own recipe end to end -- table, batched insert, publish
/// `query` -- then scores completion with `gold.call_check` against the
/// real structured result of the task's own question.
async fn run_task_once(gold: &Gold, rows: &[Value], envs_dir: &TempDataDir) -> Completion {
    let start = std::time::Instant::now();

    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;

    let signup_client = McpClient::new(&server.base_url).with_client_info(SYNTHORG_CLIENT_NAME, "1.0.0");
    let signup_resp = signup_client
        .tools_call("signup", json!({"name": "database-in-a-minute-owner"}))
        .await
        .expect("signup");
    let signup_struct = extract_structured(&signup_resp);
    let key = signup_struct["key"].as_str().expect("key").to_string();
    let client = McpClient::with_bearer(&server.base_url, &key).with_client_info(SYNTHORG_CLIENT_NAME, "1.0.0");

    client
        .tools_call(
            "host.state.table_create",
            json!({
                "name": "expenses",
                "schema": {"id": "integer", "category": "text", "amount": "real", "day": "integer"},
                "primary_key": "id",
            }),
        )
        .await
        .expect("table create");

    for batch in rows.chunks(BATCH_SIZE) {
        client
            .tools_call("host.state.insert", json!({"table": "expenses", "rows": batch}))
            .await
            .expect("insert batch");
    }

    let query_source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/database-in-a-minute/tools/query.py"),
    )
    .expect("read query.py");
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": gold.tool_name, "kind": gold.kind, "spec": {"source": query_source}}),
        )
        .await
        .expect("publish query");

    let query_qualified = format!(
        "{}.{}",
        signup_struct["tenant"].as_str().expect("tenant"),
        gold.tool_name
    );
    let answered = poll_until_ready(
        &client,
        &query_qualified,
        json!({"where": [{"col": "category", "op": "=", "value": "produce"}]}),
        Duration::from_secs(10),
    )
    .await
    .unwrap_or_else(|e| panic!("query must succeed: {} {}", e.code, e.message));
    let structured = extract_structured(&answered);

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
    assert_eq!(gold.tool_name, "query");
    assert_eq!(gold.kind, "python");

    let rows = fixture_rows();
    assert_eq!(rows.len(), 1000, "fixture.csv must have exactly 1000 data rows");

    let envs_dir = TempDataDir::new();
    let mut completions = Vec::new();
    for _ in 0..5 {
        completions.push(run_task_once(&gold, &rows, &envs_dir).await);
    }

    assert_eq!(completions.len(), 5, "expected 5 completions from 5 runs of the synthorg task");
    for completion in &completions {
        assert!(completion.wall_time_ms > 0, "each completion must record a wall time");
    }
}
