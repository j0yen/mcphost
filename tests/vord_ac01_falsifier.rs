//! PRD-mcphost-declared-ordering-per-verb
//! AC1 (P0) -- Given a local daemon and a writer inserting 200 documents and
//! 200 messages during a `limit: 25` walk, When `vord_ac01_falsifier` runs 20
//! trials of `host.docs.list` and `host.msg.inbox` on HEAD before this PRD's
//! code, Then it records duplicates and skips to `target/vord-census.json`
//! and fails with `premise_false` only if both are 0 in all 20 trials AND
//! `host.runs.list(verdict=…)` never under-fills a page.
//!
//! The run-list leg is measured two ways so the falsifier stays meaningful
//! after R4 lands: the live `host.runs.list(verdict, limit)` call, and the
//! pre-PRD "wider window then post-filter" algorithm replayed over the same
//! rows (fetch the newest `limit * 10` runs, keep the matching ones, re-cap).
//! The second one is code-independent: it is the premise, not the fix.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup, signup_and_make_pro, vord_dups_and_skips, vord_seed_runs, vord_walk};
use serde_json::{Value, json};
use std::time::Duration;

const TRIALS: usize = 20;
const PRE_EXISTING: usize = 100;
const WRITTEN: usize = 200;
const LIMIT: u64 = 25;
const PACE: Duration = Duration::from_millis(5);

#[derive(Default)]
struct Tally {
    dups: usize,
    skips: usize,
    trials_with_defect: usize,
}

impl Tally {
    fn record(&mut self, dups: usize, skips: usize) {
        self.dups += dups;
        self.skips += skips;
        if dups + skips > 0 {
            self.trials_with_defect += 1;
        }
    }
}

async fn docs_trial(server: &TestServer, trial: usize) -> (usize, usize) {
    let (_ns, key, _id) = signup_and_make_pro(server, &format!("Vord AC1 docs {trial}"), &format!("cus_vord_ac1_d{trial}")).await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let mut pre = Vec::new();
    for i in 0..PRE_EXISTING {
        let name = format!("m-{i:04}");
        client.tools_call("host.docs.put", json!({"name": name, "content": "x"})).await.expect("put");
        pre.push(name);
    }
    let base = server.base_url.clone();
    let writer = tokio::spawn(async move {
        let w = McpClient::with_bearer(&base, &key);
        for i in 0..WRITTEN {
            // Names land both before and after the pre-existing ones.
            let name = if i % 2 == 0 { format!("a-{i:04}") } else { format!("z-{i:04}") };
            w.tools_call("host.docs.put", json!({"name": name, "content": "x"})).await.expect("write");
        }
    });
    let walked = vord_walk(&client, "host.docs.list", "documents", "name", json!({}), LIMIT, PACE).await;
    writer.await.expect("writer");
    vord_dups_and_skips(&walked, &pre)
}

async fn inbox_trial(server: &TestServer, trial: usize) -> (usize, usize) {
    let (_a, key_a, _) = signup_and_make_pro(server, &format!("Vord AC1 sender {trial}"), &format!("cus_vord_ac1_s{trial}")).await;
    let (ns_b, key_b) = signup(&server.base_url, &format!("Vord AC1 inbox {trial}")).await;
    let sender = McpClient::with_bearer(&server.base_url, &key_a);
    let reader = McpClient::with_bearer(&server.base_url, &key_b);
    for i in 0..PRE_EXISTING {
        sender.tools_call("host.msg.send", json!({"to": [ns_b.clone()], "body": format!("pre {i}")})).await.expect("send");
    }
    let pre = vord_walk(&reader, "host.msg.inbox", "messages", "message_id", json!({}), 100, Duration::ZERO).await;
    let base = server.base_url.clone();
    let ns = ns_b.clone();
    let writer = tokio::spawn(async move {
        let w = McpClient::with_bearer(&base, &key_a);
        for i in 0..WRITTEN {
            w.tools_call("host.msg.send", json!({"to": [ns.clone()], "body": format!("new {i}")})).await.expect("write");
        }
    });
    let walked = vord_walk(&reader, "host.msg.inbox", "messages", "message_id", json!({}), LIMIT, PACE).await;
    writer.await.expect("writer");
    vord_dups_and_skips(&walked, &pre)
}

/// Returns `(live_underfilled, legacy_window_underfilled)`.
async fn runs_underfill(server: &TestServer) -> (bool, bool) {
    let (ns, key) = signup(&server.base_url, "Vord AC1 runs").await;
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("find").expect("tenant");
    let client = McpClient::with_bearer(&server.base_url, &key);
    // 7 failing runs, all older than 53 passing ones: the newest 5 * 10 = 50
    // rows hold none of them.
    let failing = vord_seed_runs(server, tenant.id, 60, 7).await;

    let live = extract_structured(&client.tools_call("host.runs.list", json!({"verdict": "fail", "limit": 5})).await.expect("list"));
    let live_rows = live["runs"].as_array().expect("runs").len();
    let live_underfilled = live_rows < 5 && live.get("next_cursor").is_none();

    let window = extract_structured(&client.tools_call("host.runs.list", json!({"limit": 50})).await.expect("window"));
    let kept = window["runs"]
        .as_array()
        .expect("runs")
        .iter()
        .filter(|r| failing.iter().any(|f| Some(f.as_str()) == r["run_id"].as_str()))
        .count();
    (live_underfilled, kept < 5)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn vord_ac01_falsifier() {
    let server = TestServer::start_with_signup_rate_limit(10_000).await;
    let mut docs = Tally::default();
    let mut inbox = Tally::default();
    for trial in 0..TRIALS {
        let (d, s) = docs_trial(&server, trial).await;
        docs.record(d, s);
        let (d, s) = inbox_trial(&server, trial).await;
        inbox.record(d, s);
    }
    let (live_underfill, legacy_underfill) = runs_underfill(&server).await;

    let premise_false = docs.dups + docs.skips + inbox.dups + inbox.skips == 0 && !live_underfill && !legacy_underfill;
    let census: Value = json!({
        "trials": TRIALS,
        "docs_list": {"duplicates": docs.dups, "skips": docs.skips, "trials_with_defect": docs.trials_with_defect},
        "msg_inbox": {"duplicates": inbox.dups, "skips": inbox.skips, "trials_with_defect": inbox.trials_with_defect},
        "runs_list_verdict": {"live_underfill": live_underfill, "legacy_window_underfill": legacy_underfill},
        "premise_false": premise_false,
    });
    let target = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
    std::fs::create_dir_all(&target).expect("target dir");
    std::fs::write(target.join("vord-census.json"), serde_json::to_string_pretty(&census).expect("json") + "\n")
        .expect("write census");
    assert!(!premise_false, "premise_false: implicit orderings already stable and runs.list never under-fills: {census}");
}
