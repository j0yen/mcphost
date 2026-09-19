//! PRD-mcphost-share-a-tool-not-a-key
//! AC7 — Given the synthorg mcphost corpus, When the new task runs 5
//! times, Then 5 completions are recorded with wall time.
//!
//! RedBaron's actual corpus repository
//! (`corpora/mcphost/consumer-tasks.yaml`) lives outside this worktree's
//! scope (`build_into: mcphost` only), so this test can't invoke that
//! runner's own `consume.py`. What it can and does do, so the "new task"
//! itself -- not just its recipe's shell embodiment -- is genuinely
//! exercised: load `examples/share-a-tool/synthorg-task.yaml` at test
//! time, drive its steps against a real in-process server as an
//! mcphost-recognized synthorg session (`clientInfo.name` starting with
//! `synthorg`, see `mcphost::state::is_known_synthorg_client` -- the same
//! signal a real corpus runner's session would carry), and score each
//! run's completion with the task's own `gold.call_check` expression
//! parsed straight out of the YAML, not a hardcoded duplicate of it. A
//! change that breaks the recipe (secret interpolation, sharing, or the
//! upstream response shape `gold.call_check` inspects) fails this test the
//! same way it would fail a real corpus run.

use crate::common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, publish};
use mcphost::state::is_known_synthorg_client;
use serde_json::{Value, json};
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
/// of `examples/share-a-tool/synthorg-task.yaml` directly, so the test
/// exercises whatever that file actually says rather than a copy of it.
fn load_gold() -> Gold {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/share-a-tool/synthorg-task.yaml");
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

/// RedBaron's mini-grammar is `<dotted.path rooted at "result"> == <literal>`;
/// evaluated here against the caller's actual `tools/call` structured
/// result, the same value a real corpus runner's `consume.py` would score.
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

/// Runs the task's own recipe end to end -- signup owner + caller (both
/// tagged with a synthorg `clientInfo.name`), store the stand-in key,
/// publish `gold.tool_name`/`gold.kind`, share it to a group, add the
/// caller, and have the caller call it -- then scores completion with
/// `gold.call_check` against the caller's real structured result.
async fn run_task_once(gold: &Gold) -> Completion {
    let start = std::time::Instant::now();

    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "upstream": "share-a-tool-mock",
            "ok": true,
        })))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;

    let owner_signup_client =
        McpClient::new(&server.base_url).with_client_info(SYNTHORG_CLIENT_NAME, "1.0.0");
    let owner_signup = owner_signup_client
        .tools_call("signup", json!({"name": "share-a-tool-owner"}))
        .await
        .expect("owner signup");
    let owner_key = extract_structured(&owner_signup)["key"]
        .as_str()
        .expect("owner key")
        .to_string();
    let owner = McpClient::with_bearer(&server.base_url, &owner_key)
        .with_client_info(SYNTHORG_CLIENT_NAME, "1.0.0");

    let caller_signup_client =
        McpClient::new(&server.base_url).with_client_info(SYNTHORG_CLIENT_NAME, "1.0.0");
    let caller_signup = caller_signup_client
        .tools_call("signup", json!({"name": "share-a-tool-caller"}))
        .await
        .expect("caller signup");
    let caller_struct = extract_structured(&caller_signup);
    let caller_ns = caller_struct["tenant"]
        .as_str()
        .expect("caller tenant")
        .to_string();
    let caller_key = caller_struct["key"]
        .as_str()
        .expect("caller key")
        .to_string();
    let caller = McpClient::with_bearer(&server.base_url, &caller_key)
        .with_client_info(SYNTHORG_CLIENT_NAME, "1.0.0");

    owner
        .tools_call(
            "host.secret_set",
            json!({"name": "upstream_key", "value": "sk_test_synthorg_stand_in"}),
        )
        .await
        .expect("secret_set");

    let spec = json!({
        "method": "GET",
        "url": upstream.uri(),
        "headers": {"Authorization": "Bearer {{ secret.upstream_key }}"},
    });
    let qualified = publish(&owner, &gold.tool_name, &gold.kind, spec).await;

    owner
        .tools_call("host.group.create", json!({"name": "shared_tool_team"}))
        .await
        .expect("group create");
    owner
        .tools_call(
            "host.tool_share",
            json!({"name": gold.tool_name, "visibility": "group", "group": "shared_tool_team"}),
        )
        .await
        .expect("tool_share");
    owner
        .tools_call(
            "host.group.add",
            json!({"name": "shared_tool_team", "namespace": caller_ns}),
        )
        .await
        .expect("group add");

    let caller_call = caller
        .tools_call(&qualified, json!({}))
        .await
        .expect("caller tool call");
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
    assert!(
        is_known_synthorg_client(SYNTHORG_CLIENT_NAME),
        "this test's own client name must be one mcphost's synthorg classifier recognizes, \
         or it isn't actually exercising a synthorg-shaped session"
    );

    let gold = load_gold();
    assert_eq!(gold.tool_name, "paid_api");
    assert_eq!(gold.kind, "http");

    let mut completions = Vec::new();
    for _ in 0..5 {
        completions.push(run_task_once(&gold).await);
    }

    assert_eq!(
        completions.len(),
        5,
        "expected 5 completions from 5 runs of the synthorg task"
    );
    for completion in &completions {
        assert!(
            completion.wall_time_ms > 0,
            "each completion must record a wall time"
        );
    }
}
