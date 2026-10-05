//! PRD-mcphost-event-trigger-self-test
//! AC11 (P0, Live) — Given prod mcphost with this PRD deployed and
//! PRD-synthorg-truth-tier-probe-as-tenant landed, When one nightly
//! truth-tier run executes `integration-specialist-github-push-webhook-
//! handler`, Then the persona reaches a `trigger=event` run with
//! `status=done` using ≤ 1 `host.trigger.test` call and the recipe scores
//! ≥ 0.75.
//!
//! **AC11's Then is DEFERRED — operator-provisioned, and nothing in this
//! file is counted as its proof.** The number can only come from a nightly
//! `synthorg consume --tier truth` run on orch: real frontier calls billed
//! to the operator's Anthropic key (`ANTHROPIC_API_KEY` /
//! `WM_ANTHROPIC_API_KEY`, which this sandbox does not hold), driven from
//! `~/repos/synthorg` (a different repository, python, not on this gate's
//! runner box) by a timer on a live host, against `https://mcphost.dev`,
//! which does not carry this branch until the operator deploys it. The
//! deferral is declared in the PRD's own frontmatter (`deferred_acs: [11]`
//! plus a `mock_justifications` entry naming this file and the live test
//! fn) and locked by
//! `tests/mcphost_event_trigger_self_test_ac11_deferral_is_justified.rs`,
//! so it is machine-visible rather than a promise. The one test that can
//! ever claim AC11's Then is `live_truth_tier_persona_run_meets_the_target`
//! below, which fails once `MCPHOST_LIVE=1` until a post-deploy nightly's
//! own ledger row really clears every clause of AC11's bar:
//!
//!   MCPHOST_LIVE=1 [MCPHOST_TRUTH_LEDGER=<run>/ledger.jsonl] \
//!     cargo test --test suite_sandbox_NN mcphost_event_trigger_self_test_ac11
//!
//! Env vars, read only when `MCPHOST_LIVE=1`:
//!   MCPHOST_TRUTH_LEDGER      the post-deploy nightly's ledger.jsonl
//!                             (default: the newest
//!                             ~/repos/synthorg/runs/truth-tier-*/ledger.jsonl)
//!   MCPHOST_TRUTH_TRANSCRIPT  that session's own transcript .jsonl
//!                             (default: the row's own `transcript_path`,
//!                             resolved under ~/repos/synthorg)
//!
//! What the always-on tests below own is the branch's own half of AC11 —
//! the two things this branch can actually change about that nightly:
//!
//! * **The persona path itself, replayed end to end** against a real
//!   in-process mcphost:
//!   `persona_path_reaches_a_done_event_run_with_one_self_test` performs
//!   the corpus task's own steps (publish a python-kind
//!   `github_push_handler`, `host.secret_set hook-secret-1`,
//!   `host.trigger.set(kind="event", verify="github")`) and then reaches a
//!   `trigger=event` run with `status=done` satisfying the task's own
//!   committed gold — `runs.last(trigger=event).result.payload.commits ==
//!   3` — using EXACTLY ONE `host.trigger.test` call and computing no HMAC
//!   anywhere (the persona has no shell). Every `host.trigger.test` the
//!   test makes goes through a counter, so the "≤ 1" clause is mechanical.
//!   Before this branch that path could not exist: an event self-test with
//!   no signature header was rejected, and `host.runs.list` never carried
//!   a `result` to read the gold out of.
//! * **The bar the live check applies, exercised on a real nightly.** The
//!   readers and the pass/fail rule below (`ledger_row`, `self_test_calls`,
//!   `ac11_verdict`) are the same code the live test runs, and
//!   `the_pre_feature_truth_tier_night_fails_the_same_bar` runs them over
//!   committed, verbatim artifacts of the real 2026-09-29T20:06:55Z
//!   truth-tier nightly (`tests/fixtures/event-trigger-self-test/`, trimmed
//!   only of the one credential-minting `signup` line) — which fails on
//!   three counts: 3 `host_trigger_test` calls, `satisfaction 0.0`, and a
//!   final run that returned `commit_count: 3` where the committed gold
//!   reads `payload.commits` (the PRD's own open question 4). A bar that
//!   the pre-feature night passes would prove nothing; this pins that it
//!   does not.
//!
//! No number in this file is invented: both fixtures are the real nightly's
//! own bytes, and `the_fixtures_are_the_real_nightly_they_claim_to_be`
//! fails if a later edit tries to soften one.

use crate::common;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The corpus task AC11 names (`corpora/mcphost/consumer-tasks.yaml` in
/// ~/repos/synthorg), its tool name, its secret name, and its gold check —
/// quoted from the task as committed on 2026-09-29.
const TASK: &str = "integration-specialist-github-push-webhook-handler";
const TOOL: &str = "github_push_handler";
const SECRET_NAME: &str = "hook-secret-1";
const GOLD: &str = "runs.last(trigger=event).result.payload.commits == 3";
/// AC11's own two numbers.
const TARGET_SATISFACTION: f64 = 0.75;
const MAX_SELF_TESTS: usize = 1;

const FIXTURE_DIR: &str = "tests/fixtures/event-trigger-self-test";
const LEDGER_FIXTURE: &str = "ledger-20260929T200655Z-integration-specialist-02.json";
const TRANSCRIPT_FIXTURE: &str = "transcript-20260929T200655Z-integration-specialist-02.jsonl";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_fixture(name: &str) -> String {
    let path = repo_root().join(FIXTURE_DIR).join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

// -- the readers the live check runs, exercised always-on over a real night --

/// The last ledger row for `task_id` in a `synthorg consume` `ledger.jsonl`
/// (one JSON object per scored session). A single-object file — the
/// committed fixture's shape — is accepted as a one-row ledger.
fn ledger_row(ledger: &str, task_id: &str) -> Option<Value> {
    if let Ok(one) = serde_json::from_str::<Value>(ledger)
        && one["task_id"] == json!(task_id)
    {
        return Some(one);
    }
    ledger
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .rfind(|row| row["task_id"] == json!(task_id))
}

/// How many `host.trigger.test` calls the persona spent, counted off its
/// own transcript (`{"role":"tool","tool":"host_trigger_test",...}` — one
/// line per tool call, the shape synthorg writes).
fn self_test_calls(transcript: &str) -> usize {
    transcript
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|line| line["role"] == json!("tool") && line["tool"] == json!("host_trigger_test"))
        .count()
}

/// Synthorg's own live-gold normalisation for a run result
/// (`consume.py`, "`view["payload"]` is what live-gold's gold checks are
/// written against"): an envelope carrying a `payload` key resolves to it;
/// for a python-kind tool — this task's `gold.kind` — the envelope IS the
/// object `main` returned. Only these two rules are used here; the `http`
/// rule cannot apply to a python-kind task.
///
/// Rebasing mcphost-event-trigger-self-test onto mcphost-dry-run-side-
/// effects (2026-10-03, this rebase): every `host.trigger.test` delivery
/// is itself a dry run (PRD-mcphost-dry-run-side-effects AC3 -- a
/// self-test never delivers for real), so the run row this test reads
/// back carries that PRD's own envelope first (`{dry_run, result,
/// schema}`, see `tests/dryrun_ac01_table_append_rolled_back.rs`), one
/// layer outside the two synthorg rules above. Synthorg's real probe
/// never sees this layer (production triggers are never dry runs), so
/// it's unwrapped here -- locally, before applying synthorg's own two
/// rules -- rather than added to either of them.
fn gold_payload(result: &Value) -> Value {
    let result = match (result.get("dry_run"), result.get("result")) {
        (Some(_), Some(inner)) => inner.clone(),
        _ => result.clone(),
    };
    match result.get("payload") {
        Some(payload) => payload.clone(),
        None => result.clone(),
    }
}

/// AC11's bar over one nightly's own artifacts: a `trigger=event` run that
/// reached `done` and satisfied the task's gold, ≤ 1 `host.trigger.test`
/// call, and satisfaction ≥ 0.75 -- plus AC11's Given that
/// PRD-synthorg-truth-tier-probe-as-tenant is landed and probing as the
/// tenant. Returns every clause that failed, so a miss says which.
fn ac11_verdict(row: &Value, transcript: &str) -> Vec<String> {
    let mut failures = Vec::new();

    let calls = self_test_calls(transcript);
    if calls > MAX_SELF_TESTS {
        failures.push(format!("{calls} host.trigger.test calls, AC11 allows {MAX_SELF_TESTS}"));
    }

    match row["satisfaction"].as_f64() {
        Some(satisfaction) if satisfaction >= TARGET_SATISFACTION => {}
        Some(satisfaction) => failures
            .push(format!("satisfaction {satisfaction}, AC11 wants >= {TARGET_SATISFACTION}")),
        None => failures.push(format!(
            "no satisfaction recorded for {TASK} (row[\"satisfaction\"] = {})",
            row["satisfaction"]
        )),
    }

    if !(row["probe_ok"] == json!(true) && row["probe_auth"] == json!("tenant")) {
        failures.push(format!(
            "probe_ok={} probe_auth={} -- AC11's Given is PRD-synthorg-truth-tier-probe-as-tenant \
             landed and probing as the tenant",
            row["probe_ok"], row["probe_auth"]
        ));
    }

    match done_event_run_meeting_the_gold(transcript) {
        Some(_) => {}
        None => failures.push(format!(
            "no terminal run in the transcript satisfies the task's gold ({GOLD})"
        )),
    }

    failures
}

/// The first terminal run readback anywhere in the transcript whose result
/// satisfies the corpus gold (`result.payload.commits == 3`, resolved
/// through [`gold_payload`]) on a `done` run. Searched recursively, because
/// a persona reads its run back however it likes -- `host.runs.wait`
/// returns the run object itself, `host.runs.list(include_result=true)`
/// returns it nested under `runs[]`.
fn done_event_run_meeting_the_gold(transcript: &str) -> Option<Value> {
    fn search(value: &Value) -> Option<Value> {
        match value {
            Value::Object(map) => {
                if value["status"] == json!("done")
                    && gold_payload(&value["result"])["commits"].as_i64() == Some(3)
                {
                    return Some(value.clone());
                }
                map.values().find_map(search)
            }
            Value::Array(items) => items.iter().find_map(search),
            _ => None,
        }
    }

    transcript
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|line| line["role"] == json!("tool"))
        .filter_map(|line| {
            let text = line["response"].as_str()?;
            serde_json::from_str::<Value>(text).ok()
        })
        .find_map(|response| search(&response))
}

// -- the branch's own half: the persona path, replayed for real -----------

/// Counts every `host.trigger.test` the replay makes, so AC11's "≤ 1
/// `host.trigger.test` call" clause is enforced by the harness rather than
/// by reading the test.
struct SelfTests<'a> {
    client: &'a common::McpClient,
    calls: usize,
}

impl<'a> SelfTests<'a> {
    fn new(client: &'a common::McpClient) -> Self {
        Self { client, calls: 0 }
    }

    async fn test(&mut self, args: Value) -> Value {
        self.calls += 1;
        extract_structured(
            &self
                .client
                .tools_call("host.trigger.test", args)
                .await
                .expect("host.trigger.test must succeed without the caller signing anything"),
        )
    }
}

#[tokio::test]
async fn persona_path_reaches_a_done_event_run_with_one_self_test() {
    // Same capability guard every python-kind test carries: real sandboxed
    // python needs unprivileged user namespaces.
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "SelfTest AC11 Integration Specialist").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    // The task: "publish a python-kind tool named `github_push_handler` for
    // GitHub-style push events". Its `main` returns the commit count under
    // the key the committed gold reads (`payload.commits`, which resolves
    // to the returned object itself for a python-kind tool).
    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": TOOL,
                "kind": "python",
                "spec": {
                    "source": "def main(args):\n    commits = args[\"event\"][\"body\"][\"commits\"]\n    return {\"commits\": len(commits)}\n",
                    "args_schema": {"type": "object"},
                    "network": "none",
                },
            }),
        )
        .await
        .expect("publish github_push_handler");

    // The persona's own "does it work at all" call (transcript turns 11/20
    // on the real night) -- and the python env's one cold start, so the
    // fired run below is not racing `tool_building`.
    let warm = poll_until_ready(
        &client,
        &format!("{ns}.{TOOL}"),
        json!({"event": {"body": {"commits": []}}}),
        Duration::from_secs(60),
    )
    .await
    .unwrap_or_else(|e| panic!("direct call must succeed: {} {}", e.code, e.message));
    assert_eq!(extract_structured(&warm)["commits"], json!(0), "warm-up: {warm:?}");

    // "set its secret to `hook-secret-1`, register the hook" -- with the
    // `github` preset this branch adds, so nothing is hand-assembled.
    client
        .tools_call("host.secret_set", json!({"name": SECRET_NAME, "value": "s3cr3t"}))
        .await
        .expect("secret_set");
    let set = extract_structured(
        &client
            .tools_call(
                "host.trigger.set",
                json!({"tool": TOOL, "kind": "event", "verify": "github", "secret": SECRET_NAME}),
            )
            .await
            .expect("trigger.set with the github preset"),
    );
    let trigger_id = set["id"].as_str().expect("trigger id").to_string();

    // "have the fixture service send one signed push event to your hook's
    // URL with that same secret" -- ONE self-test, no HMAC computed here.
    let mut self_tests = SelfTests::new(&client);
    let body = json!({
        "ref": "refs/heads/main",
        "repository": {"full_name": "synthorg/webhooks"},
        "pusher": {"name": "maya"},
        "commits": [
            {"id": "a1b2c3", "message": "fix bug"},
            {"id": "d4e5f6", "message": "add feature"},
            {"id": "789abc", "message": "update docs"},
        ],
    });
    let tested = self_tests.test(json!({"id": trigger_id, "body": body})).await;
    assert_eq!(tested["status"], json!("queued"), "trigger.test: {tested}");
    assert_eq!(tested["test"], json!(true), "trigger.test: {tested}");
    let run_id = tested["run_id"].as_str().expect("run_id").to_string();

    // "return the commit count from the event you received" -- read back
    // the way the truth-tier probe reads it: one `host.runs.list` with the
    // result inlined, no per-run `host.runs.get` fan-out.
    let deadline = Instant::now() + Duration::from_secs(60);
    let row = loop {
        let listed = extract_structured(
            &client
                .tools_call(
                    "host.runs.list",
                    json!({"trigger": "event", "include_result": true}),
                )
                .await
                .expect("runs.list"),
        );
        let row = listed["runs"]
            .as_array()
            .and_then(|rows| rows.iter().find(|r| r["run_id"] == json!(run_id)).cloned());
        match row {
            Some(row) if row["status"] == json!("done") => break row,
            other => {
                assert!(
                    Instant::now() < deadline,
                    "run {run_id} never reached done: {other:?}"
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    };

    assert_eq!(row["trigger"], json!("event"), "the fired run must be trigger=event: {row}");
    assert_eq!(
        gold_payload(&row["result"])["commits"],
        json!(3),
        "the task's committed gold is {GOLD}; row: {row}"
    );
    assert_eq!(
        self_tests.calls, MAX_SELF_TESTS,
        "AC11 allows at most {MAX_SELF_TESTS} host.trigger.test call; the replay spent {}",
        self_tests.calls
    );
}

// -- the bar, exercised on the real pre-feature nightly -------------------

#[test]
fn the_pre_feature_truth_tier_night_fails_the_same_bar() {
    let row = ledger_row(&read_fixture(LEDGER_FIXTURE), TASK)
        .unwrap_or_else(|| panic!("{LEDGER_FIXTURE} has no ledger row for {TASK}"));
    let transcript = read_fixture(TRANSCRIPT_FIXTURE);

    let failures = ac11_verdict(&row, &transcript);
    assert!(
        !failures.is_empty(),
        "the 2026-09-29T20:06:55Z nightly ran WITHOUT this branch; a bar it already passes could \
         not be AC11's bar"
    );

    let joined = failures.join("; ");
    for clause in ["host.trigger.test calls", "satisfaction", "gold"] {
        assert!(
            joined.contains(clause),
            "the pre-feature night must fail AC11's {clause:?} clause -- it really did: {joined}"
        );
    }
    // Its Given, on the other hand, was already met: the probe-as-tenant
    // PRD had landed by that night, so AC11's dependency is not what this
    // branch is waiting on.
    assert!(
        !joined.contains("probe_ok"),
        "the probe-as-tenant Given was already satisfied on that night: {joined}"
    );
}

#[test]
fn the_fixtures_are_the_real_nightly_they_claim_to_be() {
    let row = ledger_row(&read_fixture(LEDGER_FIXTURE), TASK)
        .unwrap_or_else(|| panic!("{LEDGER_FIXTURE} has no ledger row for {TASK}"));
    assert_eq!(row["persona_id"], json!("panel_integration_specialist_02"));
    assert_eq!(row["segment"], json!("integration_specialist"));
    assert_eq!(row["satisfaction"], json!(0.0), "the night's own measured number");
    assert_eq!(row["scorer_version"], json!(3));
    assert_eq!(
        row["call_error"],
        json!("url: host 'mcphost.dev' is not a publicly callable host"),
        "the night's own last error, unedited"
    );

    let transcript = read_fixture(TRANSCRIPT_FIXTURE);
    let lines: Vec<Value> = transcript
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("fixture line is not JSON: {e}")))
        .collect();
    assert_eq!(lines.len(), 31, "the session's 32 lines less its one `signup` line");
    for line in &lines {
        assert_eq!(line["role"], json!("tool"), "every kept line is a tool call: {line}");
        let tool = line["tool"].as_str().unwrap_or_default();
        assert!(
            tool.starts_with("host_"),
            "only the credential-minting `signup` line was dropped, and nothing else: {tool}"
        );
    }
    let turns: Vec<i64> = lines.iter().filter_map(|l| l["turn"].as_i64()).collect();
    assert!(
        turns.windows(2).all(|w| w[0] < w[1]) && turns.first() == Some(&2) && turns.last() == Some(&32),
        "turns 2..=32, in the session's own order: {turns:?}"
    );

    // Every one of the three self-tests carried a caller-computed
    // signature header -- the pre-branch contract this PRD removes.
    let signed_by_the_caller = lines
        .iter()
        .filter(|l| l["tool"] == json!("host_trigger_test"))
        .filter(|l| l["args"]["headers"]["X-Hub-Signature-256"].is_string())
        .count();
    assert_eq!(self_test_calls(&transcript), 3, "the night spent three self-tests");
    assert_eq!(
        signed_by_the_caller, 3,
        "all three hand-signed by the persona, because nothing else could sign them"
    );

    // And its last run did reach `done` -- with `commit_count`, not the
    // `commits` the committed gold reads (the PRD's open question 4). The
    // run existing is not the same as the gold being met.
    let terminal_results: Vec<Value> = lines
        .iter()
        .filter_map(|l| l["response"].as_str())
        .filter_map(|text| serde_json::from_str::<Value>(text).ok())
        .filter(|v| v["status"] == json!("done"))
        .map(|v| v["result"].clone())
        .collect();
    assert!(
        terminal_results.iter().any(|r| r["commit_count"] == json!(3)),
        "the third try really did return commit_count 3: {terminal_results:?}"
    );
    assert!(
        done_event_run_meeting_the_gold(&transcript).is_none(),
        "...and still missed {GOLD}, which is why the session scored 0.0"
    );
}

// -- AC11's own Then, for the operator's post-deploy nightly --------------

/// The ONLY test that can claim AC11. Skips unless `MCPHOST_LIVE=1`; once
/// set, it fails until a real post-deploy truth-tier nightly's own ledger
/// row and transcript clear every clause of AC11's bar.
#[test]
fn live_truth_tier_persona_run_meets_the_target() {
    if std::env::var("MCPHOST_LIVE").unwrap_or_default() != "1" {
        println!(
            "skip live_truth_tier_persona_run_meets_the_target: AC11 is deferred \
             (operator-provisioned -- a nightly `synthorg consume --tier truth` run on orch \
             against a prod deployment carrying this branch). Set MCPHOST_LIVE=1 \
             [MCPHOST_TRUTH_LEDGER=<run>/ledger.jsonl] to run it."
        );
        return;
    }

    let ledger_path = match std::env::var("MCPHOST_TRUTH_LEDGER") {
        Ok(p) if !p.is_empty() => PathBuf::from(p),
        _ => newest_truth_tier_ledger().unwrap_or_else(|| {
            panic!(
                "no ~/repos/synthorg/runs/truth-tier-*/ledger.jsonl found; point \
                 MCPHOST_TRUTH_LEDGER at the post-deploy nightly's ledger"
            )
        }),
    };
    let ledger = std::fs::read_to_string(&ledger_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", ledger_path.display()));
    let row = ledger_row(&ledger, TASK).unwrap_or_else(|| {
        panic!("{} has no ledger row for {TASK}", ledger_path.display())
    });

    let transcript_path = match std::env::var("MCPHOST_TRUTH_TRANSCRIPT") {
        Ok(p) if !p.is_empty() => PathBuf::from(p),
        _ => {
            let rel = row["transcript_path"].as_str().unwrap_or_else(|| {
                panic!("ledger row for {TASK} carries no transcript_path; set MCPHOST_TRUTH_TRANSCRIPT")
            });
            synthorg_root().join(rel)
        }
    };
    let transcript = std::fs::read_to_string(&transcript_path).unwrap_or_else(|e| {
        panic!(
            "read {}: {e} -- AC11 counts `host.trigger.test` calls, which only the session's own \
             transcript records; set MCPHOST_TRUTH_TRANSCRIPT",
            transcript_path.display()
        )
    });

    let failures = ac11_verdict(&row, &transcript);
    assert!(
        failures.is_empty(),
        "AC11 not met by {}: {}",
        ledger_path.display(),
        failures.join("; ")
    );
}

fn synthorg_root() -> PathBuf {
    match std::env::var("HOME") {
        Ok(home) => PathBuf::from(home).join("repos/synthorg"),
        Err(_) => PathBuf::from("/repos/synthorg"),
    }
}

fn newest_truth_tier_ledger() -> Option<PathBuf> {
    let runs = synthorg_root().join("runs");
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(runs)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("truth-tier-"))
        })
        .map(|p| p.join("ledger.jsonl"))
        .filter(|p| p.is_file())
        .collect();
    candidates.sort();
    candidates.pop()
}
