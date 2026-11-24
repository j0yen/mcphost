//! PRD-mcphost-response-size-budget-explicit-v2
//! AC2 (P0) -- Given each verb in `PAGED_VERBS` with enough rows to pass an
//! 8 KiB plan budget, When it is read at its maximum `limit` and every
//! `continue_with` is followed verbatim, Then each page is within 8 KiB or is
//! one row, ends on a whole row, `continue_with.args.limit` equals the
//! request's `limit`, and the walk visits every row once; Given
//! `host.channel.read {ack: true}` cut, Then the stored position is the last
//! row kept.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};
use std::collections::BTreeSet;

const BUDGET: usize = 8 * 1024;

/// Read `verb` with `base`, follow every `continue_with` verbatim, and return
/// the id of every row visited, in order.
async fn walk(client: &McpClient, verb: &str, base: Value, key: &str, id_of: impl Fn(&Value) -> String) -> Vec<String> {
    let request_limit = base["limit"].clone();
    assert!(request_limit.is_number(), "{verb}: the walk asks for an explicit (maximum) limit");
    let mut args = base.clone();
    let mut ids = Vec::new();
    let mut cut_pages = 0;
    for page_no in 0..200 {
        let result = client.tools_call(verb, args.clone()).await.unwrap_or_else(|e| panic!("{verb} page {page_no}: {e:?}"));
        let body = extract_structured(&result);
        let rows = body[key].as_array().unwrap_or_else(|| panic!("{verb}: no {key}: {body}"));
        let size = serde_json::to_vec(rows).expect("rows serialize").len();
        assert!(size <= BUDGET || rows.len() == 1, "{verb} page {page_no}: {size} bytes over {BUDGET} in {} rows", rows.len());
        for row in rows {
            assert!(row.is_object(), "{verb}: a page ends on whole rows: {row}");
            ids.push(id_of(row));
        }
        match result["_meta"]["mcphost"].get("truncated") {
            Some(marker) => {
                cut_pages += 1;
                let next = &marker["continue_with"];
                assert_eq!(next["verb"], json!(verb), "{marker}");
                assert_eq!(next["args"]["limit"], request_limit, "{verb}: continue_with keeps the request's limit: {marker}");
                assert_eq!(next["args"]["cursor"], body["next_cursor"], "{verb}: {marker}");
                assert_eq!(marker["bytes"], json!(size), "{verb}: bytes is the page sent: {marker}");
                assert!(marker["of"].as_u64().expect("of") > size as u64, "{verb}: {marker}");
                let mut without_cursor = next["args"].clone();
                without_cursor.as_object_mut().expect("args object").remove("cursor");
                assert_eq!(without_cursor, base, "{verb}: continue_with is the request plus a cursor");
                args = next["args"].clone();
            }
            None => {
                assert!(body.get("next_cursor").is_none(), "{verb}: an unmarked page at max limit is the last: {body}");
                assert!(cut_pages >= 1, "{verb}: the fixture never passed the budget ({size} bytes)");
                let unique: BTreeSet<&String> = ids.iter().collect();
                assert_eq!(unique.len(), ids.len(), "{verb}: a row was visited twice");
                return ids;
            }
        }
    }
    panic!("{verb}: the walk did not terminate");
}

fn id_field(field: &'static str) -> impl Fn(&Value) -> String {
    move |row| row[field].to_string()
}

async fn server_with_budget() -> TestServer {
    TestServer::start_with_plans(|catalog| {
        for plan in &mut catalog.plans {
            plan.response_budget_bytes = BUDGET as i64;
            plan.msgs_per_hour = 100_000;
            plan.calls_per_day = 1_000_000;
        }
    })
    .await
}

fn expect_set(verb: &str, seen: &[String], expected: BTreeSet<String>) {
    let got: BTreeSet<String> = seen.iter().cloned().collect();
    assert_eq!(got.len(), seen.len(), "{verb}: a row was visited twice");
    assert_eq!(got, expected, "{verb}: the walk visits every row once");
}

#[tokio::test]
async fn rszb_ac02_every_paged_verb_walks_within_budget() {
    let server = server_with_budget().await;
    let (ns, key) = signup(&server.base_url, "Rszb AC2 A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Rszb AC2 B").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let other = McpClient::with_bearer(&server.base_url, &key_b);
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("db lookup").expect("tenant exists");
    let padding = "p".repeat(600);
    let mut covered = BTreeSet::new();

    // host.runs.list
    let mut expected = BTreeSet::new();
    for i in 0..60 {
        let id = format!("rszb-ac2-run-{i:03}");
        server
            .state
            .db
            .insert_queued_run(id.clone(), tenant.id, "ghost".into(), "event".into(), None, None, 300, "{}".into(), false, false, None, None, None, None)
            .await
            .expect("insert_queued_run");
        expected.insert(json!(id).to_string());
    }
    let seen = walk(&client, "host.runs.list", json!({"limit": 200}), "runs", id_field("run_id")).await;
    expect_set("host.runs.list", &seen, expected);
    covered.insert("host.runs.list");

    // host.docs.list
    let mut expected = BTreeSet::new();
    for i in 0..80 {
        let name = format!("doc-{i:03}-{}", "n".repeat(60));
        client.tools_call("host.docs.put", json!({"name": name, "content": "x"})).await.expect("put");
        expected.insert(json!(name).to_string());
    }
    let seen = walk(&client, "host.docs.list", json!({"limit": 1000}), "documents", id_field("name")).await;
    expect_set("host.docs.list", &seen, expected);
    covered.insert("host.docs.list");

    // host.calls.list (rows identified by their one-second timestamp)
    let now = mcphost::state::now_unix();
    let mut expected = BTreeSet::new();
    for i in 0..120 {
        server.state.db.insert_calls_row_for_test(tenant.id, now - 60 * (i + 1)).await.expect("calls row");
        expected.insert(json!(mcphost::state::rfc3339_from_unix(now - 60 * (i + 1))).to_string());
    }
    let seen = walk(&client, "host.calls.list", json!({"limit": 500, "tool": "test_tool", "since": "-30d"}), "calls", id_field("ts")).await;
    expect_set("host.calls.list", &seen, expected);
    covered.insert("host.calls.list");

    // host.msg.inbox and host.msg.thread
    let mut expected = BTreeSet::new();
    let mut thread_id = String::new();
    for i in 0..30 {
        let sent = extract_structured(
            &client.tools_call("host.msg.send", json!({"to": [ns_b], "body": format!("m{i:02} {padding}")})).await.expect("send"),
        );
        expected.insert(json!(sent["message_id"]).to_string());
        if thread_id.is_empty() {
            thread_id = sent["thread_id"].as_str().expect("thread_id").to_string();
        }
    }
    let seen = walk(&other, "host.msg.inbox", json!({"limit": 100}), "messages", id_field("message_id")).await;
    expect_set("host.msg.inbox", &seen, expected);
    covered.insert("host.msg.inbox");

    let mut expected = BTreeSet::new();
    let first = extract_structured(&other.tools_call("host.msg.thread", json!({"thread_id": thread_id, "limit": 100})).await.expect("thread"));
    expected.insert(json!(first["messages"][0]["message_id"]).to_string());
    for i in 0..30 {
        let reply = extract_structured(
            &other.tools_call("host.msg.reply", json!({"thread_id": thread_id, "body": format!("r{i:02} {padding}")})).await.expect("reply"),
        );
        expected.insert(json!(reply["message_id"]).to_string());
    }
    let seen = walk(&other, "host.msg.thread", json!({"thread_id": thread_id, "limit": 100}), "messages", id_field("message_id")).await;
    expect_set("host.msg.thread", &seen, expected);
    covered.insert("host.msg.thread");

    // host.channel.read
    let opened = extract_structured(&client.tools_call("host.channel.open", json!({"name": "walk"})).await.expect("open"));
    let channel = opened["channel_id"].as_str().expect("channel_id").to_string();
    let mut expected = BTreeSet::new();
    for i in 0..30 {
        let post = extract_structured(
            &client.tools_call("host.channel.post", json!({"channel": channel, "body": format!("c{i:02} {padding}")})).await.expect("post"),
        );
        expected.insert(json!(post["post_id"]).to_string());
    }
    let seen = walk(&client, "host.channel.read", json!({"channel_id": channel, "limit": 100}), "posts", id_field("post_id")).await;
    expect_set("host.channel.read", &seen, expected);
    covered.insert("host.channel.read");

    // Every verb the host pages is walked here.
    let paged: BTreeSet<&str> = mcphost::paged::PAGED_VERBS.iter().copied().collect();
    assert_eq!(covered, paged, "a new paged verb needs a walk in this test");
}

#[tokio::test]
async fn rszb_ac02_a_row_over_the_budget_is_returned_alone_and_ack_stores_the_last_row_kept() {
    let server = server_with_budget().await;
    let (_ns, key) = signup(&server.base_url, "Rszb AC2 Ack").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let opened = extract_structured(&client.tools_call("host.channel.open", json!({"name": "ack"})).await.expect("open"));
    let channel = opened["channel_id"].as_str().expect("channel_id").to_string();
    let padding = "q".repeat(600);
    for i in 0..30 {
        client
            .tools_call("host.channel.post", json!({"channel": channel, "body": format!("a{i:02} {padding}")}))
            .await
            .expect("post");
    }

    // A cut read with ack stores the last row kept, not the last row fetched.
    let cut = client.tools_call("host.channel.read", json!({"channel_id": channel, "limit": 100, "ack": true})).await.expect("read");
    let body = extract_structured(&cut);
    let kept = body["posts"].as_array().expect("posts");
    assert!(cut["_meta"]["mcphost"]["truncated"].is_object(), "{cut:?}");
    assert!(kept.len() < 30, "{} rows kept", kept.len());
    let last_kept = kept.last().expect("a row")["seq"].as_i64().expect("seq");
    let resumed = extract_structured(&client.tools_call("host.channel.read", json!({"channel_id": channel, "limit": 1})).await.expect("resume"));
    assert_eq!(resumed["posts"][0]["seq"], json!(last_kept + 1), "the dropped rows are read again: {resumed}");

    // One row larger than the whole budget is sent alone.
    let big = extract_structured(&client.tools_call("host.channel.open", json!({"name": "big"})).await.expect("open"));
    let big_channel = big["channel_id"].as_str().expect("channel_id").to_string();
    for body in ["small-1".to_string(), "B".repeat(20 * 1024), "small-2".to_string()] {
        client.tools_call("host.channel.post", json!({"channel": big_channel, "body": body})).await.expect("post");
    }
    let mut sizes = Vec::new();
    let mut args = json!({"channel_id": big_channel, "limit": 100});
    for _ in 0..5 {
        let result = client.tools_call("host.channel.read", args.clone()).await.expect("read");
        sizes.push(extract_structured(&result)["posts"].as_array().expect("posts").len());
        match result["_meta"]["mcphost"].get("truncated") {
            Some(marker) => args = marker["continue_with"]["args"].clone(),
            None => break,
        }
    }
    assert_eq!(sizes, [1, 1, 1], "small, then the oversized row alone, then small");
}
