//! PRD-mcphost-chain-run-lineage AC13 (P0, Live) -- "Given prod mcphost
//! with this PRD deployed and PRD-synthorg-truth-tier-probe-as-tenant
//! landed, When one nightly truth-tier run executes
//! `data-pipeline-builder-daily-pipeline-chain`, Then (1) the persona's
//! first `host.tool_call` of `daily_pipeline` either succeeds or fails
//! `compose_input_missing` and (2) its second succeeds, (3) the probe
//! observes `runs.last(tool=daily_pipeline).children.count == 3`, and (4)
//! the recipe scores >= 0.75 (live: nightly `synthorg consume --tier truth`
//! on orch)."
//!
//! Conjuncts 1, 2 and 3 are this file's subject, and they are proven, not
//! stood in for. Conjunct 4 -- the recipe's own score -- is split: its
//! arithmetic is `synthorg`'s, it is ported here, and it is run against
//! this branch's real observation, so what stays deferred is one value
//! inside it rather than the whole conjunct.
//!
//! `_classify_gold_agreement` scores a session `timeliness * accuracy *
//! helpfulness`, and `_recipe_satisfaction` takes the mean over the
//! recipe's scored sessions. `helpfulness` is one of three judge LABELS
//! (`HELPFULNESS_LABEL_VALUES`: blocked 0.0 / partly 0.5 / helped 1.0) and
//! `_accuracy_score` is one of three values too, so 0.75 is decidable
//! without knowing the judge's verdict: 0.5 * 1.0 * 1.0 = 0.5 < 0.75, so
//! the bar is reachable ONLY when `accuracy == 1.0`, which takes the gold
//! tool listed AND its `call_check` observed True. That observation is
//! exactly what this branch supplies. So this file proves, against a real
//! server, that (a) with this branch the session's accuracy factor is
//! 1.0 and the recipe's score is exactly the nightly's own judge label,
//! and (b) with the `children` this PRD added stripped back off the very
//! same payload it is 0.5, putting the recipe below target at every judge
//! label and every timeliness -- the 0.00 the grounding runs recorded.
//!
//! What is left deferred, and all that is left: the judge's own label and
//! the persona's own wall clock from a paid, operator-authorized `synthorg
//! consume --tier truth` run on host orch against a prod deployment of
//! this branch. Neither is a property of this branch. See the PRD
//! frontmatter's `mock_justifications` and
//! `tests/mcphost_chain_run_lineage_ac13_deferral_is_justified.rs`.
//!
//! Conjunct 3 is the one that needed real product work, and it is why this
//! file is not just a persona re-enactment. "The probe" is a concrete piece
//! of code: `synthorg`'s `_emit_host_context_probe` (src/synthorg/
//! consume.py), which after the session closes opens one fresh connection
//! as the session's own tenant and makes exactly four calls -- `tools/list`,
//! `host.trigger.list`, `host.table.list`, and ONE `host.runs.list` with no
//! arguments -- then evaluates the corpus gold
//! (`corpora/mcphost/consumer-tasks.yaml:811`) off those rows alone. It
//! never calls `host.runs.get`. Against this PRD's first implementation it
//! could therefore never observe a chain's lineage at all: `host.runs.list`
//! rows carried no `children` key, so `.children.count` read 0 for every
//! run, and the recipe would have scored 0.00 on the operator's dime no
//! matter how many child rows `compose_call` wrote. `src/runs.rs`'s `list`
//! now inlines each row's own children, one level, exactly as `get` does.
//!
//! The probe's second half is `_last_matching_run`, which reads
//! `RunCheckContext.runs` with `reversed(...)` -- "last" means the last
//! recorded row. mcphost answers `host.runs.list` newest-first (`ORDER BY
//! rowid DESC`), so a verbatim recording made `runs.last(tool=
//! daily_pipeline)` resolve to the OLDEST matching run: for this persona,
//! its own refused first call, which by AC3 has no children by
//! construction. That half is fixed in synthorg, where the probe lives, by
//! `normalize_probed_runs` + `_emit_host_context_probe(runs_order=...)`
//! (`/home/jsy/repos/synthorg`, committed on master, unpushed -- the sha is
//! recorded in `agent/test-map.json`'s AC13 entry, and the Python proof is
//! `tests/chainlineage_ac13_probe_runs_order_test.py`).
//!
//! So this file ports the probe -- its four calls, its runs-order
//! normalization, its one-level `children` fold, its `reversed(...)` "last",
//! and its gold grammar for `runs.last(<selector>).children.count <op> <n>`
//! -- and drives the persona's own publish/call/call sequence through it
//! against a real `mcphost` server speaking streamable-HTTP JSON-RPC: no
//! mock, no stub, no in-process shortcut. What the environment selects is
//! only *which* endpoint:
//!
//! * Default (every `cargo test` run, including this build's gate): a real
//!   server process bound on an ephemeral `127.0.0.1` port
//!   ([`TestServer`]), serving the same `mcphost::http` app `main.rs`
//!   serves in production, with a freshly signed-up tenant.
//! * `MCPHOST_LIVE=1`: the identical sequence against the real
//!   `$MCPHOST_URL` (default `https://mcphost.dev`), also with a fresh
//!   signup -- the post-ship trailer run, which can only pass once the
//!   operator has deployed this branch.
//!
//! What stays unproven here, and only here: that the nightly run's own LLM
//! judge returns `helped` for `data-pipeline-builder-daily-pipeline-chain`
//! and that its persona first-calls the chain inside the timeliness
//! window. Those are the two factors of conjunct 4's product that belong
//! to the operator's money and the persona's latency; the third --
//! `accuracy`, and with it whether the bar is reachable at all -- is the
//! branch's, and is asserted below.

use std::time::{Duration, Instant};

use crate::common;
use common::{TestServer, chain_kind_registry, extract_structured, publish, signup};
use serde_json::{Value, json};

/// `corpora/mcphost/consumer-tasks.yaml:811` -- the
/// `data-pipeline-builder-daily-pipeline-chain` recipe's own `call_check`,
/// verbatim, the string AC13's conjunct 3 quotes.
const GOLD: &str = "runs.last(tool=daily_pipeline).children.count == 3";

/// `synthorg`'s `_PROBE_REQUIRED_TOOLS`: the tool names
/// `_emit_host_context_probe` calls, which `run_preflight` requires the
/// target endpoint to serve before a live run spends any budget.
const PROBE_REQUIRED_TOOLS: [&str; 3] =
    ["host.trigger.list", "host.runs.list", "host.table.list"];

/// Pure predicate, deliberately not reading `std::env` itself, so its
/// behavior for an unset variable is asserted deterministically instead
/// of depending on (and possibly mutating) the ambient process
/// environment.
fn live_mode_enabled(raw: Option<&str>) -> bool {
    raw == Some("1")
}

// -- the probe's own bookkeeping, ported ------------------------------------

/// Which end of a probed `host.runs.list` payload is the newest run --
/// `synthorg`'s `RUNS_ORDER_*` constants. mcphost is
/// [`RunsOrder::NewestFirst`]; synthorg's own fake endpoint (not involved
/// here) is [`RunsOrder::Chronological`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RunsOrder {
    NewestFirst,
    Chronological,
}

/// One row of `RunCheckContext.runs`, folded exactly as
/// `score_mechanical` folds a `host_context_probe` event's `runs`: a
/// child keeps only its `trigger`/`status` (plus, here, its `tool`/
/// `step_no`, which the trailer prints), and no child of a child is ever
/// read -- the fold stops at one level, matching the grammar.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ProbeRun {
    tool: Option<String>,
    trigger: String,
    status: String,
    step_no: Option<i64>,
    children: Vec<ProbeRun>,
}

fn probe_run_from_row(row: &Value, fold_children: bool) -> ProbeRun {
    ProbeRun {
        tool: row["tool"].as_str().map(str::to_string),
        trigger: row["trigger"].as_str().unwrap_or_default().to_string(),
        status: row["status"].as_str().unwrap_or_default().to_string(),
        step_no: row["step_no"].as_i64(),
        children: if fold_children {
            row["children"]
                .as_array()
                .map(|rows| rows.iter().map(|c| probe_run_from_row(c, false)).collect())
                .unwrap_or_default()
        } else {
            Vec::new()
        },
    }
}

/// `normalize_probed_runs` + `score_mechanical`'s fold in one step: the
/// `runs` array of a `host.runs.list` result, recorded in chronological
/// (append) order -- which for mcphost means reversed, since it answers
/// newest-first. A row with no `children` key at all folds to no children,
/// the pre-fix shape (and the shape any other host would answer).
fn record_probed_runs(payload: &Value, order: RunsOrder) -> Vec<ProbeRun> {
    let mut rows: Vec<ProbeRun> = payload["runs"]
        .as_array()
        .map(|rows| rows.iter().map(|r| probe_run_from_row(r, true)).collect())
        .unwrap_or_default();
    if order == RunsOrder::NewestFirst {
        rows.reverse();
    }
    rows
}

/// `_parse_kv_args` for a `runs.last(...)` selector: `tool=daily_pipeline`,
/// `tool=reindex_chain, status=done`. Values are unquoted the same way.
fn parse_selector(raw: &str) -> Vec<(String, String)> {
    raw.split(',')
        .filter_map(|part| part.split_once('='))
        .map(|(k, v)| {
            let v = v.trim();
            let v = v.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap_or(v);
            (k.trim().to_string(), v.to_string())
        })
        .collect()
}

/// `_matches_run_selector`: every named key must equal the row's own
/// value, by exact string comparison (which is why a run row's `tool` has
/// to be the bare published name the corpus writes, not a namespaced one).
fn matches_run_selector(run: &ProbeRun, selector: &[(String, String)]) -> bool {
    selector.iter().all(|(key, want)| match key.as_str() {
        "tool" => run.tool.as_deref() == Some(want.as_str()),
        "trigger" => &run.trigger == want,
        "status" => &run.status == want,
        other => panic!("selector key '{other}' is not one of synthorg's _RUN_SELECTOR_KEYS"),
    })
}

/// `_last_matching_run`: the LAST recorded matching row -- `reversed(...)`
/// over a chronological list, i.e. the most recent matching run.
fn last_matching_run<'a>(runs: &'a [ProbeRun], selector: &[(String, String)]) -> Option<&'a ProbeRun> {
    runs.iter().rev().find(|run| matches_run_selector(run, selector))
}

/// `eval_run_shaped_check`'s `_RUN_LAST_CHILDREN_RE` branch: evaluate
/// `runs.last(<selector>).children.count <op> <value>` against a recorded
/// context. `None` when `check` is not that shape at all (the port covers
/// exactly the one form AC13 quotes); `Some(false)` -- never a panic --
/// when no run matches the selector, mirroring the grammar's own
/// "recognized but unobservable reads False" rule.
fn eval_last_children_count(check: &str, runs: &[ProbeRun]) -> Option<bool> {
    let rest = check.trim().strip_prefix("runs.last(")?;
    let (selector_raw, rest) = rest.split_once(").children.count")?;
    let rest = rest.trim();
    let (op, value) = ["==", "!=", ">=", "<=", ">", "<"]
        .into_iter()
        .find_map(|op| rest.strip_prefix(op).map(|v| (op, v)))?;
    let want: usize = value.trim().parse().ok()?;
    let selector = parse_selector(selector_raw);
    let Some(run) = last_matching_run(runs, &selector) else {
        return Some(false);
    };
    let got = run.children.len();
    Some(match op {
        "==" => got == want,
        "!=" => got != want,
        ">=" => got >= want,
        "<=" => got <= want,
        ">" => got > want,
        _ => got < want,
    })
}

// -- the truth-tier scorer, ported ------------------------------------------
//
// Conjunct 4 says "the recipe scores >= 0.75". That number is not opaque:
// `synthorg`'s scorer computes it from three factors, one of which is the
// gold observation above. Porting the arithmetic is what lets this file
// say exactly how much of conjunct 4 is the branch's and how much is the
// operator's money -- instead of deferring the whole conjunct because a
// judge is involved somewhere inside it.

/// `RECIPE_SATISFACTION_TARGET` (`src/synthorg/consume.py`): the 0.75 bar
/// AC13's conjunct 4 names, and the value `_recipe_satisfaction` compares
/// a recipe's mean scored satisfaction against when it builds
/// `measure.json`'s `recipes.below_target` list.
const RECIPE_SATISFACTION_TARGET: f64 = 0.75;

/// `HELPFULNESS_LABEL_VALUES` (`consume.py`: `{"blocked": 0.0, "partly":
/// 0.5, "helped": 1.0}`). The judge returns a LABEL, not a free float --
/// there are exactly three values `helpfulness` can ever take, which is
/// what makes conjunct 4's arithmetic decidable below without knowing what
/// the operator's judge will say.
const HELPFULNESS_LABELS: [(&str, f64); 3] = [("blocked", 0.0), ("partly", 0.5), ("helped", 1.0)];

/// `TIMELINESS_FULL_CREDIT_SECONDS` / `TIMELINESS_ZERO_SECONDS`
/// (`consume.py`; the latter is `SESSION_WALL_BUDGET_SECONDS = 300.0`).
const TIMELINESS_FULL_CREDIT_SECONDS: f64 = 60.0;
const TIMELINESS_ZERO_SECONDS: f64 = 300.0;

/// `_timeliness_score`: how long after `initialize` the persona first
/// called its own tool. A property of the persona's latency, not of
/// mcphost -- except for mcphost's own share of that wall clock, measured
/// for real below.
fn timeliness_score(t_initialize: Option<f64>, t_first_own_call: Option<f64>) -> f64 {
    let (Some(t_initialize), Some(t_first_own_call)) = (t_initialize, t_first_own_call) else {
        return 0.0;
    };
    let elapsed = t_first_own_call - t_initialize;
    if elapsed <= TIMELINESS_FULL_CREDIT_SECONDS {
        return 1.0;
    }
    if elapsed >= TIMELINESS_ZERO_SECONDS {
        return 0.0;
    }
    let span = TIMELINESS_ZERO_SECONDS - TIMELINESS_FULL_CREDIT_SECONDS;
    1.0 - (elapsed - TIMELINESS_FULL_CREDIT_SECONDS) / span
}

/// `_accuracy_score`: 1.0 when the gold tool was listed and its
/// `call_check` holds, 0.5 when listed and the check ran but came back
/// False, 0.0 when no call of it ever succeeded so the check never ran.
/// This is the one factor of the three that this branch moves, and the
/// whole of its reach into conjunct 4's score.
fn accuracy_score(own_tool_listed: bool, call_check_result: Option<bool>) -> f64 {
    match (own_tool_listed, call_check_result) {
        (false, _) | (_, None) => 0.0,
        (true, Some(true)) => 1.0,
        (true, Some(false)) => 0.5,
    }
}

/// `_classify_gold_agreement`'s ordinary return: one session's
/// `satisfaction` is the product of the three factors.
fn session_satisfaction(timeliness: f64, accuracy: f64, helpfulness: f64) -> f64 {
    timeliness * accuracy * helpfulness
}

/// `_recipe_satisfaction`: a recipe's score is the MEAN satisfaction over
/// its scored sessions (`sum(scored_vals) / len(scored_vals)`), `None`
/// when it had none.
fn recipe_score(scored_sessions: &[f64]) -> Option<f64> {
    if scored_sessions.is_empty() {
        return None;
    }
    Some(scored_sessions.iter().sum::<f64>() / scored_sessions.len() as f64)
}

/// The complement of `_recipe_satisfaction`'s `below_target` test: a
/// recipe clears AC13's bar when its mean scored satisfaction is not under
/// `RECIPE_SATISFACTION_TARGET`.
fn recipe_meets_target(scored_sessions: &[f64]) -> bool {
    recipe_score(scored_sessions).is_some_and(|score| score >= RECIPE_SATISFACTION_TARGET)
}

/// `_matches_gold` over an already-canonical listed name: mcphost serves
/// `t_<8hex>.<name>`, which is exactly the spelling
/// `canonicalize_tool_name` normalizes to, so the port is the comparison
/// itself.
fn matches_gold(listed: &str, gold_tool_name: &str, namespace: &str) -> bool {
    listed == gold_tool_name || listed == format!("{namespace}.{gold_tool_name}")
}

// -- the endpoint under test ------------------------------------------------

/// The endpoint this run drives, plus the fresh tenant identity behind it.
struct Target {
    label: String,
    base_url: String,
    key: String,
    ns: String,
    /// Kept alive for the test's duration in the default path; `None`
    /// when driving a real remote endpoint.
    _local: Option<TestServer>,
}

impl Target {
    /// `MCPHOST_LIVE=1`: a fresh tenant on the real `$MCPHOST_URL`,
    /// exactly as the truth-tier persona signs up before every session.
    async fn live() -> Self {
        let base_url = std::env::var("MCPHOST_URL").unwrap_or_else(|_| "https://mcphost.dev".into());
        let base_url = base_url.trim_end_matches("/mcp").to_string();
        let (ns, key) = signup(&base_url, "AC13 truth-tier persona").await;
        Self { label: format!("live {base_url}"), base_url, key, ns, _local: None }
    }

    /// The default: a real `mcphost` server on a real loopback socket,
    /// with a fresh tenant signed up -- the state prod is in when the
    /// nightly check starts.
    async fn local() -> Self {
        let server = TestServer::start_with_kinds(chain_kind_registry()).await;
        let (ns, key) = signup(&server.base_url, "AC13 Tenant").await;
        Self {
            label: format!("server at {}", server.base_url),
            base_url: server.base_url.clone(),
            key,
            ns,
            _local: Some(server),
        }
    }

    async fn resolve() -> Self {
        if live_mode_enabled(std::env::var("MCPHOST_LIVE").ok().as_deref()) {
            Self::live().await
        } else {
            Self::local().await
        }
    }
}

/// One `host_context_probe` event, as `_emit_host_context_probe` builds it:
/// the endpoint's tool names, its `triggers`/`tables`/`runs` read-backs,
/// and the overall `probe_ok` that goes `False` the moment any one of the
/// four calls errors.
struct Probe {
    tools: Vec<String>,
    runs_payload: Value,
    probe_ok: bool,
}

async fn emit_host_context_probe(client: &common::McpClient) -> Probe {
    let mut probe_ok = true;
    let tools = match client.tools_list().await {
        Ok(listed) => listed["tools"]
            .as_array()
            .map(|t| t.iter().filter_map(|t| t["name"].as_str().map(str::to_string)).collect())
            .unwrap_or_default(),
        Err(_) => {
            probe_ok = false;
            Vec::new()
        }
    };
    for name in ["host.trigger.list", "host.table.list"] {
        if client.tools_call(name, json!({})).await.is_err() {
            probe_ok = false;
        }
    }
    // The probe's one and only runs read: no filters, no `include_children`,
    // no follow-up `host.runs.get`.
    let runs_payload = match client.tools_call("host.runs.list", json!({})).await {
        Ok(result) => extract_structured(&result),
        Err(_) => {
            probe_ok = false;
            json!({"runs": []})
        }
    };
    Probe { tools, runs_payload, probe_ok }
}

/// AC13 conjuncts 1-3, plus conjunct 4's branch-local half -- drives the
/// persona's own publish/call/call sequence against a real endpoint, reads
/// it back through the ported truth-tier probe, asserts the recipe's own
/// gold string is True, and then runs that observation through the ported
/// scorer: `accuracy == 1.0` here, `0.5` on the same payload with this
/// PRD's `children` stripped off, and 0.75 is out of reach at every judge
/// label and every timeliness in the stripped case. Runs on every `cargo
/// test`; `MCPHOST_LIVE=1` only redirects it at real prod.
#[tokio::test]
async fn persona_two_call_sequence_yields_three_done_children() {
    let target = Target::resolve().await;
    let client = common::McpClient::with_bearer(&target.base_url, &target.key);

    // `_timeliness_score`'s own clock: `t_initialize` is the moment this
    // session's MCP connection is usable, `t_first_own_call` the moment the
    // persona first calls its own gold tool. Measured for real so the
    // trailer can say how much of the 60 s full-credit window mcphost's own
    // share of the sequence actually spends.
    let t_initialize = Instant::now();

    let schema = json!({"type": "object"});
    publish(&client, "fetch_data", "echo", json!({"schema": schema})).await;
    publish(&client, "transform", "echo", json!({"schema": schema})).await;
    publish(&client, "write", "echo", json!({"schema": schema})).await;

    // The `echo` kind returns its own args back as its result, so step 2's
    // mapping reads step 1's `url` field (not `rows`) -- same shape as
    // `tests/mcphost_chain_run_lineage_ac04_child_run_rows_per_step.rs`,
    // which actually executes this chain for real.
    let chain_spec = json!({
        "steps": [
            {"tool": "fetch_data", "args": {"url": "$.input.url"}},
            {"tool": "transform", "args": {"rows": "$.prev.result.url"}},
            {"tool": "write", "args": {"rows": "$.prev.result.rows", "region": "$.input.region"}},
        ]
    });
    let chain = publish(&client, "daily_pipeline", "chain", chain_spec).await;
    assert_eq!(chain, format!("{}.daily_pipeline", target.ns));

    // When (conjunct 1): the persona's first call, exactly as the grounding
    // transcripts show (`host_tool_call {"name":"daily_pipeline",
    // "args":{}}`) -- AC13 allows either outcome for it; this branch's
    // schema-derived pre-check makes an empty call fail before step 1 runs.
    let first = client.tools_call(&chain, json!({})).await;
    let t_first_own_call = t_initialize.elapsed().as_secs_f64();
    if let Err(e) = &first {
        assert_eq!(
            e.error_code.as_deref(),
            Some("compose_input_missing"),
            "a first call missing inputs must fail this named error, not something else: {e:?}"
        );
    }

    // Conjunct 2: its second call, with both inputs, must succeed -- no
    // republish, same chain, same mapping (the workaround the grounding
    // transcripts show in 4 of 4 sessions is what this PRD removes).
    let second = client
        .tools_call(&chain, json!({"url": "https://example.com/data", "region": "eu"}))
        .await
        .unwrap_or_else(|e| panic!("the persona's second call must succeed: {} {}", e.code, e.message));
    let structured = extract_structured(&second);
    assert!(
        structured["failed_steps"].as_array().map(|a| a.is_empty()).unwrap_or(true),
        "every step of the successful call must succeed: {structured}"
    );

    // Then (conjunct 3): the truth-tier probe's own read-back. Child rows
    // are written by `compose_call` during the parent call, so on a healthy
    // server they are already there; the probe gets one bounded retry loop
    // rather than a single racy read, since the nightly one runs after the
    // session closes.
    let deadline = Instant::now() + Duration::from_secs(30);
    let (probe, recorded) = loop {
        let probe = emit_host_context_probe(&client).await;
        let recorded = record_probed_runs(&probe.runs_payload, RunsOrder::NewestFirst);
        if eval_last_children_count(GOLD, &recorded) == Some(true) {
            break (probe, recorded);
        }
        assert!(
            Instant::now() < deadline,
            "the probe never observed `{GOLD}`; it recorded {recorded:#?} from {}",
            probe.runs_payload
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    assert!(probe.probe_ok, "every one of the probe's four calls must succeed");
    for required in PROBE_REQUIRED_TOOLS {
        assert!(
            probe.tools.iter().any(|t| t == required),
            "`run_preflight` refuses to spend budget unless the endpoint serves {required}: {:?}",
            probe.tools
        );
    }

    let observed = last_matching_run(&recorded, &parse_selector("tool=daily_pipeline"))
        .expect("the gold matched, so a run matched");
    assert_eq!(observed.status, "done", "the run the probe reads must be the successful one");
    assert_eq!(observed.children.len(), 3);
    assert_eq!(
        observed.children.iter().map(|c| c.status.as_str()).collect::<Vec<_>>(),
        ["done", "done", "done"]
    );
    assert!(observed.children.iter().all(|c| c.trigger == "composition"));
    assert_eq!(
        observed.children.iter().filter_map(|c| c.tool.as_deref()).collect::<Vec<_>>(),
        ["fetch_data", "transform", "write"],
        "children come back in step order"
    );
    assert_eq!(
        observed.children.iter().filter_map(|c| c.step_no).collect::<Vec<_>>(),
        [1, 2, 3]
    );

    // The two inversions, against this very payload, so the proof states
    // what it depends on: strip the `children` this PRD added to
    // `host.runs.list` rows and the probe reads 0 children (the shape
    // before `src/runs.rs`'s `list` inlined them); record mcphost's
    // newest-first answer verbatim and `runs.last` lands on the refused
    // first call instead (the shape before synthorg's probe normalized the
    // order). Both make the gold False.
    let mut stripped = probe.runs_payload.clone();
    for row in stripped["runs"].as_array_mut().expect("runs array") {
        row.as_object_mut().expect("run row object").remove("children");
    }
    assert_eq!(
        eval_last_children_count(GOLD, &record_probed_runs(&stripped, RunsOrder::NewestFirst)),
        Some(false),
        "without `children` on the listed rows the probe can observe no lineage at all"
    );
    if first.is_err() {
        let verbatim = record_probed_runs(&probe.runs_payload, RunsOrder::Chronological);
        assert_eq!(
            eval_last_children_count(GOLD, &verbatim),
            Some(false),
            "recorded newest-first, `runs.last` resolves to the refused first call: {verbatim:#?}"
        );
    }

    // Then (conjunct 4's branch-local half): the recipe's SCORE, through
    // the ported scorer, off this very observation. `satisfaction =
    // timeliness * accuracy * helpfulness`, and the judge's `helpfulness`
    // is one of three labels -- so the 0.75 bar is decidable here without
    // knowing the operator's judge's verdict. `accuracy` can only be 1.0,
    // 0.5 or 0.0, and 0.5 * 1.0 * 1.0 = 0.5 < 0.75: the bar is reachable
    // ONLY with `accuracy == 1.0`, which needs the gold tool listed AND its
    // `call_check` observed True. That observation is precisely what this
    // branch supplies and nothing else in the session can.
    let own_tool_listed =
        probe.tools.iter().any(|t| matches_gold(t, "daily_pipeline", &target.ns));
    assert!(
        own_tool_listed,
        "`_matches_gold` must find the gold tool in the probe's own tools/list: {:?}",
        probe.tools
    );

    let accuracy = accuracy_score(own_tool_listed, eval_last_children_count(GOLD, &recorded));
    assert_eq!(
        accuracy, 1.0,
        "the gold this branch made observable is what makes the accuracy factor whole"
    );
    let stripped_accuracy = accuracy_score(
        own_tool_listed,
        eval_last_children_count(GOLD, &record_probed_runs(&stripped, RunsOrder::NewestFirst)),
    );
    assert_eq!(
        stripped_accuracy, 0.5,
        "without the inlined `children` the gold tool is still listed and still called \
         successfully, so the session lands on `_accuracy_score`'s partial credit -- not 0"
    );

    for (label, helpfulness) in HELPFULNESS_LABELS {
        for timeliness in [0.0, 0.25, 0.5, 0.75, 1.0] {
            // Without this PRD: below the bar at EVERY judge label and
            // EVERY timeliness, so `data-pipeline-builder-daily-pipeline-
            // chain` lands in `measure.json`'s `below_target` however well
            // the persona's session reads -- which is the 0.00 the
            // grounding run recorded on 09-28 and 09-29.
            let without = session_satisfaction(timeliness, stripped_accuracy, helpfulness);
            assert!(
                without < RECIPE_SATISFACTION_TARGET,
                "0.5 accuracy caps satisfaction at 0.5: judge said {label:?}, \
                 timeliness {timeliness}, satisfaction {without}"
            );
            assert!(
                !recipe_meets_target(&[without, without]),
                "the recipe's mean over the corpus's own n=2 sessions is below target too"
            );
        }
        // With it: the product's factor is spent, and the session's score
        // is exactly the nightly's own timeliness times its own judge
        // label. Nothing mcphost does can lower it further.
        let with_branch = session_satisfaction(1.0, accuracy, helpfulness);
        assert_eq!(with_branch, helpfulness, "accuracy 1.0 leaves the judge's label untouched");
        assert_eq!(
            with_branch >= RECIPE_SATISFACTION_TARGET,
            label == "helped",
            "with accuracy whole, the bar turns on the judge's label alone: {label:?}"
        );
    }

    // And the two-session recipe mean AC13 is actually scored on, for the
    // outcome the nightly is run to get: a "helped" verdict on a session
    // that first-called its own tool inside the full-credit window.
    let helped = 1.0;
    let nightly = session_satisfaction(
        timeliness_score(Some(0.0), Some(t_first_own_call)),
        accuracy,
        helped,
    );
    assert!(
        recipe_meets_target(&[nightly, nightly]),
        "score {nightly} over n=2 sessions must clear {RECIPE_SATISFACTION_TARGET}"
    );

    // mcphost's own share of the timeliness clock: the whole
    // publish/publish/publish/publish/call sequence, server-side, against a
    // real socket. The persona's thinking time is the operator's to spend;
    // the product's share must leave the full-credit window intact.
    assert!(
        t_first_own_call < TIMELINESS_FULL_CREDIT_SECONDS,
        "mcphost's own share of the session ({t_first_own_call:.3}s) must fit inside \
         `_timeliness_score`'s {TIMELINESS_FULL_CREDIT_SECONDS}s full-credit window"
    );

    // The trailer's own evidence line.
    println!(
        "AC13 conjuncts 1-3 ({}) -- probe observed `{GOLD}` = true; \
         runs.last(tool=daily_pipeline).children = {}\n\
         AC13 conjunct 4 (branch-local half) -- accuracy {accuracy} (was {stripped_accuracy} \
         with `children` stripped), mcphost's share of the timeliness clock {t_first_own_call:.3}s \
         of {TIMELINESS_FULL_CREDIT_SECONDS}s; recipe score = the nightly judge's own label. \
         Unproven here and only here: that label, from a paid `synthorg consume --tier truth` \
         run on orch.",
        target.label,
        serde_json::to_string_pretty(&probe.runs_payload["runs"][0]["children"]).unwrap_or_default()
    );
}

#[test]
fn live_mode_is_disabled_when_mcphost_live_is_unset_or_not_1() {
    assert!(!live_mode_enabled(None), "unset must disable live mode");
    assert!(!live_mode_enabled(Some("")), "empty must disable live mode");
    assert!(!live_mode_enabled(Some("0")), "MCPHOST_LIVE=0 must disable live mode");
    assert!(!live_mode_enabled(Some("true")), "only the literal '1' enables live mode");
    assert!(live_mode_enabled(Some("1")), "MCPHOST_LIVE=1 must enable live mode");
}

/// The ported grammar, unit-tested against the exact payload shape a real
/// mcphost build answers for this persona's session (two `daily_pipeline`
/// parents in the same second, newest first: the successful retry with
/// three inlined `done` children, then the refused first call with none),
/// so the port's own decisions are asserted rather than trusted.
fn mcphost_two_parent_payload() -> Value {
    json!({"runs": [
        {
            "run_id": "01M3QSVNQDZTPEGS592ZCHE0PW", "tool": "daily_pipeline",
            "trigger": "call", "status": "done", "parent_run_id": null,
            "children": [
                {"tool": "fetch_data", "trigger": "composition", "status": "done", "step_no": 1},
                {"tool": "transform", "trigger": "composition", "status": "done", "step_no": 2},
                {"tool": "write", "trigger": "composition", "status": "done", "step_no": 3},
            ],
        },
        {
            "run_id": "01M3QSVNQ99SH8A19R9FG9TVQ5", "tool": "daily_pipeline",
            "trigger": "call", "status": "error", "parent_run_id": null,
            "error_class": "compose_input_missing", "children": [],
        },
    ]})
}

#[test]
fn the_ported_gold_grammar_reads_the_most_recent_matching_run() {
    let recorded = record_probed_runs(&mcphost_two_parent_payload(), RunsOrder::NewestFirst);
    assert_eq!(
        recorded.iter().map(|r| r.status.as_str()).collect::<Vec<_>>(),
        ["error", "done"],
        "recording normalizes mcphost's newest-first answer to chronological order"
    );
    assert_eq!(eval_last_children_count(GOLD, &recorded), Some(true));
    // Verbatim (what the probe did before synthorg normalized the order):
    // `runs.last` is the refused first call, which has no children.
    let verbatim = record_probed_runs(&mcphost_two_parent_payload(), RunsOrder::Chronological);
    assert_eq!(eval_last_children_count(GOLD, &verbatim), Some(false));
    assert_eq!(
        last_matching_run(&verbatim, &parse_selector("tool=daily_pipeline")).unwrap().status,
        "error"
    );
}

#[test]
fn the_ported_gold_grammar_fails_honestly_on_every_near_miss() {
    // No `children` key at all -- `host.runs.list` before this PRD.
    let mut no_children = mcphost_two_parent_payload();
    for row in no_children["runs"].as_array_mut().unwrap() {
        row.as_object_mut().unwrap().remove("children");
    }
    assert_eq!(
        eval_last_children_count(GOLD, &record_probed_runs(&no_children, RunsOrder::NewestFirst)),
        Some(false)
    );
    // Two children, not three: a chain that stopped early still fails.
    let mut short = mcphost_two_parent_payload();
    short["runs"][0]["children"].as_array_mut().unwrap().pop();
    assert_eq!(
        eval_last_children_count(GOLD, &record_probed_runs(&short, RunsOrder::NewestFirst)),
        Some(false)
    );
    // No run at all for the selector: False, never a panic.
    assert_eq!(eval_last_children_count(GOLD, &[]), Some(false));
    // A selector the corpus's sibling recipe uses (`:829`) narrows by
    // status, so the refused call cannot be selected in the first place.
    let recorded = record_probed_runs(&mcphost_two_parent_payload(), RunsOrder::Chronological);
    assert_eq!(
        eval_last_children_count(
            "runs.last(tool=daily_pipeline, status=done).children.count == 3",
            &recorded
        ),
        Some(true)
    );
    // Other comparison operators, and a check outside this one form.
    assert_eq!(
        eval_last_children_count(
            "runs.last(tool=daily_pipeline).children.count >= 3",
            &record_probed_runs(&mcphost_two_parent_payload(), RunsOrder::NewestFirst)
        ),
        Some(true)
    );
    assert_eq!(eval_last_children_count("runs.count(trigger=composition) == 3", &[]), None);
    // A child's own `children` is never read: the fold stops at one level.
    let mut nested = mcphost_two_parent_payload();
    nested["runs"][0]["children"][0]["children"] = json!([{"status": "done"}]);
    let recorded = record_probed_runs(&nested, RunsOrder::NewestFirst);
    assert!(
        last_matching_run(&recorded, &parse_selector("tool=daily_pipeline"))
            .unwrap()
            .children
            .iter()
            .all(|c| c.children.is_empty())
    );
}

/// The ported scorer's own decisions, asserted rather than trusted --
/// every branch of `_timeliness_score` and `_accuracy_score`, the
/// three-factor product, and `_recipe_satisfaction`'s mean-over-scored-
/// sessions, against the values `src/synthorg/consume.py` computes for the
/// same inputs.
#[test]
fn the_ported_scorer_matches_synthorgs_own_arithmetic() {
    // `_timeliness_score`: no clock at all scores 0, not full credit.
    assert_eq!(timeliness_score(None, Some(1.0)), 0.0);
    assert_eq!(timeliness_score(Some(0.0), None), 0.0);
    // Inside full credit, at the boundary, and past the wall budget.
    assert_eq!(timeliness_score(Some(0.0), Some(0.5)), 1.0);
    assert_eq!(timeliness_score(Some(0.0), Some(TIMELINESS_FULL_CREDIT_SECONDS)), 1.0);
    assert_eq!(timeliness_score(Some(0.0), Some(TIMELINESS_ZERO_SECONDS)), 0.0);
    assert_eq!(timeliness_score(Some(0.0), Some(600.0)), 0.0);
    // Linear in between: 180s is halfway from 60s to 300s.
    assert_eq!(timeliness_score(Some(0.0), Some(180.0)), 0.5);
    // It reads a DIFFERENCE, not an absolute timestamp.
    assert_eq!(timeliness_score(Some(1_000.0), Some(1_030.0)), 1.0);

    // `_accuracy_score`: never listed, or a check that never ran, is 0 --
    // AC-12's "listed, but no call of it ever succeeded" included.
    assert_eq!(accuracy_score(false, Some(true)), 0.0);
    assert_eq!(accuracy_score(false, None), 0.0);
    assert_eq!(accuracy_score(true, None), 0.0);
    assert_eq!(accuracy_score(true, Some(false)), 0.5);
    assert_eq!(accuracy_score(true, Some(true)), 1.0);

    // The product, and the judge's three possible labels.
    assert_eq!(
        HELPFULNESS_LABELS.map(|(_, v)| v),
        [0.0, 0.5, 1.0],
        "the judge returns a label; there is no fourth value for it to take"
    );
    assert_eq!(session_satisfaction(1.0, 1.0, 1.0), 1.0);
    assert_eq!(session_satisfaction(1.0, 0.5, 1.0), 0.5);
    assert_eq!(session_satisfaction(0.5, 1.0, 1.0), 0.5);
    assert_eq!(session_satisfaction(1.0, 1.0, 0.0), 0.0);

    // `_recipe_satisfaction`: the mean, and the target comparison.
    assert_eq!(recipe_score(&[]), None);
    assert!(!recipe_meets_target(&[]), "a recipe with no scored session never clears the bar");
    assert_eq!(recipe_score(&[1.0, 0.5]), Some(0.75));
    assert!(recipe_meets_target(&[1.0, 0.5]), "the bar is >=, so an exact 0.75 clears it");
    assert!(!recipe_meets_target(&[1.0, 0.0]), "a mean of 0.5 is below target");

    // `_matches_gold` over mcphost's own spelling.
    assert!(matches_gold("t_0a1b2c3d.daily_pipeline", "daily_pipeline", "t_0a1b2c3d"));
    assert!(matches_gold("daily_pipeline", "daily_pipeline", "t_0a1b2c3d"));
    assert!(!matches_gold("t_deadbeef.daily_pipeline", "daily_pipeline", "t_0a1b2c3d"));
    assert!(!matches_gold("t_0a1b2c3d.fetch_data", "daily_pipeline", "t_0a1b2c3d"));
}

/// Conjunct 4's arithmetic, stated as the claim the deferral rests on: the
/// 0.75 bar is UNREACHABLE without the `children` this PRD inlines into
/// `host.runs.list` rows -- at every judge label and every timeliness --
/// and with them the score is exactly the judge's own label. So the only
/// thing a paid nightly run can still decide is that label; the product's
/// half of conjunct 4 is settled here, against the payload a real mcphost
/// build answers with.
#[test]
fn the_recipe_bar_is_unreachable_without_the_children_this_prd_inlines() {
    let with_children = record_probed_runs(&mcphost_two_parent_payload(), RunsOrder::NewestFirst);
    let mut no_children_payload = mcphost_two_parent_payload();
    for row in no_children_payload["runs"].as_array_mut().unwrap() {
        row.as_object_mut().unwrap().remove("children");
    }
    let without_children = record_probed_runs(&no_children_payload, RunsOrder::NewestFirst);

    // The gold tool is listed and its call succeeded in BOTH worlds -- the
    // only thing that differs is whether the probe can observe the
    // lineage.
    let after = accuracy_score(true, eval_last_children_count(GOLD, &with_children));
    let before = accuracy_score(true, eval_last_children_count(GOLD, &without_children));
    assert_eq!((before, after), (0.5, 1.0));

    for (label, helpfulness) in HELPFULNESS_LABELS {
        for timeliness in [0.0, 0.1, 0.5, 0.75, 0.9, 1.0] {
            let scored = session_satisfaction(timeliness, before, helpfulness);
            assert!(
                scored < RECIPE_SATISFACTION_TARGET,
                "before this PRD the recipe cannot clear {RECIPE_SATISFACTION_TARGET} even with \
                 judge={label:?} and timeliness={timeliness}: {scored}"
            );
            // And at every session count the nightly might run: every
            // session in a run sees the same product, so the recipe's mean
            // is this value however many of them there are.
            for n in [1usize, 2, 3, 5] {
                assert!(
                    !recipe_meets_target(&vec![scored; n]),
                    "n={n} sessions all scoring {scored} still mean below target"
                );
            }
        }
        // After: the session's score IS the judge's label, at full
        // timeliness, and the bar turns on that label alone.
        let scored = session_satisfaction(1.0, after, helpfulness);
        assert_eq!(scored, helpfulness);
        assert_eq!(recipe_meets_target(&[scored; 2]), label == "helped");
    }

    // The one arrangement the nightly is run to reach, spelled out: two
    // "helped" sessions inside the full-credit window score 1.00.
    let nightly = session_satisfaction(timeliness_score(Some(0.0), Some(12.0)), after, 1.0);
    assert_eq!(recipe_score(&[nightly; 2]), Some(1.0));
    assert!(recipe_meets_target(&[nightly; 2]));

    // A chain that only got two steps done still misses the bar, so the
    // assertion above is about the lineage being CORRECT, not merely
    // present.
    let mut short = mcphost_two_parent_payload();
    short["runs"][0]["children"].as_array_mut().unwrap().pop();
    let short_accuracy = accuracy_score(
        true,
        eval_last_children_count(GOLD, &record_probed_runs(&short, RunsOrder::NewestFirst)),
    );
    assert_eq!(short_accuracy, 0.5);
    assert!(!recipe_meets_target(&[session_satisfaction(1.0, short_accuracy, 1.0); 2]));
}
