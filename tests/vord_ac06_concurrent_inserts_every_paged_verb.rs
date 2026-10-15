//! PRD-mcphost-declared-ordering-per-verb
//! AC6 (P0) -- Given every `Paged` verb with a fixture, When the walk
//! (`vord_ac05` in the PRD's numbering) runs it under concurrent inserts
//! under each declared order for 20 trials, Then zero duplicate ids and zero
//! skipped pre-existing ids.
//!
//! Each trial builds a fresh fixture, records the ids that exist before the
//! walk, starts a writer task inserting 200 more rows, and walks `limit: 25`
//! to the end. The union must hold no id twice and every pre-existing id.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup, signup_and_make_pro, vord_dups_and_skips, vord_seed_runs, vord_walk};
use mcphost::paged::{CallsOrder, ChannelOrder, DocsOrder, InboxOrder, OrderEnum, PAGED_VERBS, RunsOrder, ThreadOrder};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;

const TRIALS: usize = 20;
const PRE_EXISTING: usize = 100;
const WRITTEN: usize = 200;
const LIMIT: u64 = 25;
const PACE: Duration = Duration::from_millis(2);

struct Fixture {
    reader: McpClient,
    array_key: &'static str,
    id_key: &'static str,
    args: Value,
    pre: Vec<String>,
    writer: JoinHandle<()>,
}

/// Every id the reader can see before the writer starts, under `order`.
async fn snapshot(reader: &McpClient, verb: &str, array_key: &str, id_key: &str, args: &Value) -> Vec<String> {
    vord_walk(reader, verb, array_key, id_key, args.clone(), 100, Duration::ZERO).await
}

async fn docs_fixture(server: &Arc<TestServer>, tag: &str, order: &str) -> Fixture {
    let (_ns, key, _id) = signup_and_make_pro(server, &format!("Vord AC6 docs {tag}"), &format!("cus_vord_ac6_d_{tag}")).await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    for i in 0..PRE_EXISTING {
        client.tools_call("host.docs.put", json!({"name": format!("m-{i:04}"), "content": "x"})).await.expect("put");
    }
    let args = json!({"order": order});
    let pre = snapshot(&client, "host.docs.list", "documents", "name", &args).await;
    let base = server.base_url.clone();
    let writer = tokio::spawn(async move {
        let w = McpClient::with_bearer(&base, &key);
        for i in 0..WRITTEN {
            let name = if i % 2 == 0 { format!("a-{i:04}") } else { format!("z-{i:04}") };
            w.tools_call("host.docs.put", json!({"name": name, "content": "x"})).await.expect("write");
        }
    });
    Fixture { reader: client, array_key: "documents", id_key: "name", args, pre, writer }
}

async fn inbox_fixture(server: &Arc<TestServer>, tag: &str) -> Fixture {
    let (_a, key_a, _) = signup_and_make_pro(server, &format!("Vord AC6 inbox sender {tag}"), &format!("cus_vord_ac6_i_{tag}")).await;
    let (ns_b, key_b) = signup(&server.base_url, &format!("Vord AC6 inbox {tag}")).await;
    let sender = McpClient::with_bearer(&server.base_url, &key_a);
    for i in 0..PRE_EXISTING {
        sender.tools_call("host.msg.send", json!({"to": [ns_b.clone()], "body": format!("pre {i}")})).await.expect("send");
    }
    let reader = McpClient::with_bearer(&server.base_url, &key_b);
    let args = json!({});
    let pre = snapshot(&reader, "host.msg.inbox", "messages", "message_id", &args).await;
    let base = server.base_url.clone();
    let writer = tokio::spawn(async move {
        let w = McpClient::with_bearer(&base, &key_a);
        for i in 0..WRITTEN {
            w.tools_call("host.msg.send", json!({"to": [ns_b.clone()], "body": format!("new {i}")})).await.expect("write");
        }
    });
    Fixture { reader, array_key: "messages", id_key: "message_id", args, pre, writer }
}

async fn thread_fixture(server: &Arc<TestServer>, tag: &str) -> Fixture {
    let (_a, key_a, _) = signup_and_make_pro(server, &format!("Vord AC6 thread A {tag}"), &format!("cus_vord_ac6_t_{tag}")).await;
    let (ns_b, key_b) = signup(&server.base_url, &format!("Vord AC6 thread B {tag}")).await;
    let a = McpClient::with_bearer(&server.base_url, &key_a);
    let sent = extract_structured(&a.tools_call("host.msg.send", json!({"to": [ns_b], "body": "first"})).await.expect("send"));
    let thread_id = sent["thread_id"].as_str().expect("thread_id").to_string();
    for i in 1..PRE_EXISTING {
        a.tools_call("host.msg.reply", json!({"thread_id": thread_id, "body": format!("pre {i}")})).await.expect("reply");
    }
    let reader = McpClient::with_bearer(&server.base_url, &key_b);
    let args = json!({"thread_id": thread_id});
    let pre = snapshot(&reader, "host.msg.thread", "messages", "message_id", &args).await;
    let base = server.base_url.clone();
    let tid = thread_id.clone();
    let writer = tokio::spawn(async move {
        let w = McpClient::with_bearer(&base, &key_a);
        for i in 0..WRITTEN {
            w.tools_call("host.msg.reply", json!({"thread_id": tid, "body": format!("new {i}")})).await.expect("write");
        }
    });
    Fixture { reader, array_key: "messages", id_key: "message_id", args, pre, writer }
}

async fn channel_fixture(server: &Arc<TestServer>, tag: &str) -> Fixture {
    let (_ns, key, _id) = signup_and_make_pro(server, &format!("Vord AC6 channel {tag}"), &format!("cus_vord_ac6_c_{tag}")).await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let opened = extract_structured(&client.tools_call("host.channel.open", json!({"name": format!("vord-{tag}")})).await.expect("open"));
    let channel = opened["channel_id"].as_str().expect("channel_id").to_string();
    for i in 0..PRE_EXISTING {
        client.tools_call("host.channel.post", json!({"channel": channel, "body": format!("pre {i}")})).await.expect("post");
    }
    let args = json!({"channel_id": channel});
    let pre = snapshot(&client, "host.channel.read", "posts", "post_id", &args).await;
    let base = server.base_url.clone();
    let ch = channel.clone();
    let writer = tokio::spawn(async move {
        let w = McpClient::with_bearer(&base, &key);
        for i in 0..WRITTEN {
            w.tools_call("host.channel.post", json!({"channel": ch, "body": format!("new {i}")})).await.expect("write");
        }
    });
    Fixture { reader: client, array_key: "posts", id_key: "post_id", args, pre, writer }
}

async fn runs_fixture(server: &Arc<TestServer>, tag: &str, order: &str) -> Fixture {
    let (ns, key) = signup(&server.base_url, &format!("Vord AC6 runs {tag}")).await;
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("find").expect("tenant");
    vord_seed_runs(server, tenant.id, PRE_EXISTING, 0).await;
    let reader = McpClient::with_bearer(&server.base_url, &key);
    let args = json!({"order": order});
    let pre = snapshot(&reader, "host.runs.list", "runs", "run_id", &args).await;
    let state = server.state.clone();
    let writer = tokio::spawn(async move {
        for _ in 0..WRITTEN {
            state
                .db
                .insert_done_run_with_progress_for_test(tenant.id, "vord_echo".to_string(), json!({"budget": {"verdict": "ok"}}).to_string())
                .await
                .expect("write run");
            tokio::task::yield_now().await;
        }
    });
    Fixture { reader, array_key: "runs", id_key: "run_id", args, pre, writer }
}

async fn calls_fixture(server: &Arc<TestServer>, tag: &str) -> Fixture {
    let (ns, key) = signup(&server.base_url, &format!("Vord AC6 calls {tag}")).await;
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("find").expect("tenant");
    let now = mcphost::state::now_unix();
    for i in 0..PRE_EXISTING {
        server
            .state
            .db
            .insert_call_row_for_test_full(tenant.id, format!("pre{i:04}"), None, None, true, 1, now - 60)
            .await
            .expect("seed row");
    }
    let reader = McpClient::with_bearer(&server.base_url, &key);
    let args = json!({"since": "-1h"});
    let pre = snapshot(&reader, "host.calls.list", "calls", "tool", &args).await;
    let state = server.state.clone();
    let writer = tokio::spawn(async move {
        for i in 0..WRITTEN {
            state
                .db
                .insert_call_row_for_test_full(tenant.id, format!("new{i:04}"), None, None, true, 1, now - 30)
                .await
                .expect("write row");
            tokio::task::yield_now().await;
        }
    });
    Fixture { reader, array_key: "calls", id_key: "tool", args, pre, writer }
}

async fn fixture(server: &Arc<TestServer>, verb: &str, order: &str, tag: &str) -> Fixture {
    match verb {
        "host.docs.list" => docs_fixture(server, tag, order).await,
        "host.msg.inbox" => inbox_fixture(server, tag).await,
        "host.msg.thread" => thread_fixture(server, tag).await,
        "host.channel.read" => channel_fixture(server, tag).await,
        "host.runs.list" => runs_fixture(server, tag, order).await,
        "host.calls.list" => calls_fixture(server, tag).await,
        other => panic!("{other} derives Paged but has no vord_ac06 fixture"),
    }
}

/// `(verb, declared orders)`, generated from each verb's order enum.
fn declared() -> Vec<(&'static str, Vec<&'static str>)> {
    vec![
        ("host.docs.list", DocsOrder::names()),
        ("host.msg.inbox", InboxOrder::names()),
        ("host.msg.thread", ThreadOrder::names()),
        ("host.channel.read", ChannelOrder::names()),
        ("host.runs.list", RunsOrder::names()),
        ("host.calls.list", CallsOrder::names()),
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_paged_verb_walks_clean_under_concurrent_inserts_in_every_declared_order() {
    let covered: Vec<&str> = declared().iter().map(|(v, _)| *v).collect();
    for verb in PAGED_VERBS {
        assert!(covered.contains(verb), "{verb} derives Paged but is not in this test's declared() table");
    }

    let server = Arc::new(TestServer::start_with_signup_rate_limit(100_000).await);
    let mut cases = Vec::new();
    for (verb, orders) in declared() {
        for order in orders {
            let server = server.clone();
            cases.push(tokio::spawn(async move {
                let (mut dups, mut skips) = (0, 0);
                for trial in 0..TRIALS {
                    let tag = format!("{}-{order}-{trial}", verb.replace('.', "_"));
                    let f = fixture(&server, verb, order, &tag).await;
                    assert!(f.pre.len() >= PRE_EXISTING, "{verb} {order}: fixture holds {} rows", f.pre.len());
                    let walked = vord_walk(&f.reader, verb, f.array_key, f.id_key, f.args.clone(), LIMIT, PACE).await;
                    f.writer.await.expect("writer");
                    let (d, s) = vord_dups_and_skips(&walked, &f.pre);
                    dups += d;
                    skips += s;
                }
                (verb, order, dups, skips)
            }));
        }
    }
    let mut failures = Vec::new();
    for case in cases {
        let (verb, order, dups, skips) = case.await.expect("case task");
        if dups + skips > 0 {
            failures.push(format!("{verb} order={order}: {dups} duplicate ids, {skips} skipped pre-existing ids"));
        }
    }
    assert!(failures.is_empty(), "paging under concurrent inserts: {failures:#?}");
}
