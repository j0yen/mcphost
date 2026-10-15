//! PRD-mcphost-fields-projection-on-list-verbs
//! AC4 (P0) -- Given every `Paged` verb, When `fldp_ac04_fields_one_set`
//! runs, Then `FIELDS`, the serialized row keys, the committed `fields_enum`
//! and `field_unknown.valid` are equal sets; renaming a field without
//! regenerating the contract fails the test naming the verb.
//!
//! "Every Paged verb" is `paged::projected_verbs()`: the four `Paged` verbs
//! plus `host.tool.list` / `host.runs.list`, the two windows that project.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use mcphost::paged;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

type Set = BTreeSet<String>;

fn set_of<'a>(it: impl IntoIterator<Item = &'a str>) -> Set {
    it.into_iter().map(str::to_string).collect()
}

fn value_set(v: &Value) -> Set {
    v.as_array().expect("array").iter().map(|x| x.as_str().expect("string").to_string()).collect()
}

fn row_keys(page: &Value, key: &str) -> Set {
    page[key].as_array().unwrap_or_else(|| panic!("{key} array in {page}")).iter().flat_map(|r| {
        r.as_object().expect("row object").keys().cloned().collect::<Vec<_>>()
    })
    .collect()
}

/// `Err` naming the verb unless the four sets are one set.
fn one_set(verb: &str, fields: &Set, serialized: &Set, contract: &Set, valid: &Set) -> Result<(), String> {
    for (label, other) in [("serialized row keys", serialized), ("contract fields_enum", contract), ("field_unknown.valid", valid)] {
        if other != fields {
            return Err(format!(
                "{verb}: FIELDS {fields:?} != {label} {other:?} (only in FIELDS: {:?}, only in {label}: {:?}) -- \
                 regenerate contracts/host-tools.v1.json with `mcphost contract dump`",
                fields.difference(other).collect::<Vec<_>>(),
                other.difference(fields).collect::<Vec<_>>(),
            ));
        }
    }
    Ok(())
}

fn committed_enum(verb: &str) -> Set {
    let raw = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/contracts/host-tools.v1.json"))
        .expect("committed contract");
    let contract: Value = serde_json::from_str(&raw).expect("contract json");
    let entry = contract["tools"].as_array().expect("tools").iter().find(|t| t["name"] == json!(verb));
    value_set(&entry.unwrap_or_else(|| panic!("{verb} not in contract"))["fields_enum"])
}

async fn valid_from_error(client: &McpClient, verb: &str, extra: Value) -> Set {
    let mut args = json!({"fields": ["__nope__"]});
    for (k, v) in extra.as_object().expect("object") {
        args[k] = v.clone();
    }
    let err = client.tools_call(verb, args).await.expect_err("an unknown field is refused");
    assert_eq!(err.error_code.as_deref(), Some("field_unknown"), "{verb}: {err:?}");
    value_set(&err.data["valid"])
}

#[test]
fn a_renamed_field_without_a_regenerated_contract_fails_naming_the_verb() {
    let fields = set_of(["id", "name"]);
    assert!(one_set("host.x.list", &fields, &fields, &fields, &fields).is_ok());
    let renamed = set_of(["id", "title"]);
    let msg = one_set("host.x.list", &fields, &renamed, &fields, &fields).expect_err("drift is caught");
    assert!(msg.starts_with("host.x.list:"), "{msg}");
    assert!(msg.contains("serialized row keys"), "{msg}");
    let msg = one_set("host.x.list", &fields, &fields, &renamed, &fields).expect_err("stale contract is caught");
    assert!(msg.contains("contract fields_enum"), "{msg}");
}

#[tokio::test]
async fn fields_serialized_keys_contract_enum_and_error_valid_are_one_set_for_every_verb() {
    let server = TestServer::start_with_signup_rate_limit(20).await;
    let (_ns_a, key_a) = common::signup(&server.base_url, "Fldp AC4 A").await;
    let (ns_b, key_b) = common::signup(&server.base_url, "Fldp AC4 B").await;
    let a = McpClient::with_bearer(&server.base_url, &key_a);
    let b = McpClient::with_bearer(&server.base_url, &key_b);

    // docs: one live, one soft-deleted (the `deleted` key shows only on a `since` walk).
    for name in ["live", "gone"] {
        a.tools_call("host.docs.put", json!({"name": name, "content": "x"})).await.expect("put");
    }
    a.tools_call("host.docs.delete", json!({"name": "gone"})).await.expect("delete");
    let mut docs = row_keys(&extract_structured(&a.tools_call("host.docs.list", json!({})).await.expect("docs")), "documents");
    docs.extend(row_keys(
        &extract_structured(&a.tools_call("host.docs.list", json!({"since": 0})).await.expect("docs since")),
        "documents",
    ));

    // channel
    let opened = extract_structured(&a.tools_call("host.channel.open", json!({"name": "fldp"})).await.expect("open"));
    let channel = opened["channel_id"].as_str().expect("channel_id").to_string();
    a.tools_call("host.channel.post", json!({"channel": channel, "body": "hi"})).await.expect("post");
    let posts = row_keys(
        &extract_structured(&a.tools_call("host.channel.read", json!({"channel_id": channel})).await.expect("read")),
        "posts",
    );

    // messages: B -> A
    let sent = extract_structured(&b.tools_call("host.msg.send", json!({"to": [ns_a_of(&a).await], "body": "hi"})).await.expect("send"));
    let thread = sent["thread_id"].as_str().expect("thread_id").to_string();
    let inbox = row_keys(&extract_structured(&a.tools_call("host.msg.inbox", json!({})).await.expect("inbox")), "messages");
    let thread_rows = row_keys(
        &extract_structured(&a.tools_call("host.msg.thread", json!({"thread_id": thread})).await.expect("thread")),
        "messages",
    );
    let _ = ns_b;

    // tools: an echo tool carries the `stub` key
    a.tools_call("host.tool.publish", json!({"name": "reflect", "kind": "echo", "spec": {"schema": {"type": "object"}}}))
        .await
        .expect("publish");
    let tools = row_keys(&extract_structured(&a.tools_call("host.tool.list", json!({})).await.expect("tools")), "tools");

    // runs: one finished event run
    let set = extract_structured(
        &a.tools_call(
            "host.trigger.set",
            json!({"tool": "reflect", "kind": "event", "verify": {"scheme": "none", "allow_unverified": true}}),
        )
        .await
        .expect("trigger.set"),
    );
    let tested = extract_structured(
        &a.tools_call("host.trigger.test", json!({"id": set["id"], "body": {"k": 1}})).await.expect("trigger.test"),
    );
    let run_id = tested["run_id"].as_str().expect("run_id").to_string();
    let deadline = Instant::now() + Duration::from_secs(60);
    while extract_structured(&a.tools_call("host.runs.get", json!({"run_id": run_id})).await.expect("get"))["status"] != json!("done") {
        assert!(Instant::now() < deadline, "run {run_id} never finished");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let runs = row_keys(&extract_structured(&a.tools_call("host.runs.list", json!({})).await.expect("runs")), "runs");

    let ns_a = ns_a_of(&a).await;
    a.tools_call(&format!("{ns_a}.reflect"), json!({})).await.expect("call reflect");
    let calls = row_keys(&extract_structured(&a.tools_call("host.calls.list", json!({})).await.expect("calls")), "calls");

    let serialized = [
        ("host.docs.list", docs, json!({})),
        ("host.calls.list", calls, json!({})),
        ("host.channel.read", posts, json!({"channel_id": channel})),
        ("host.msg.inbox", inbox, json!({})),
        ("host.msg.thread", thread_rows, json!({"thread_id": thread})),
        ("host.tool.list", tools, json!({})),
        ("host.runs.list", runs, json!({})),
    ];
    let verbs: Vec<&str> = paged::projected_verbs().collect();
    assert_eq!(verbs.len(), serialized.len(), "every projecting verb must be probed: {verbs:?}");
    for (verb, keys, extra) in serialized {
        assert!(verbs.contains(&verb), "{verb} is not a projecting verb");
        let fields = set_of(paged::fields_for(verb).unwrap_or_else(|| panic!("{verb}: no FIELDS")).iter().copied());
        let valid = valid_from_error(&a, verb, extra).await;
        if let Err(msg) = one_set(verb, &fields, &keys, &committed_enum(verb), &valid) {
            panic!("{msg}");
        }
    }
}

async fn ns_a_of(client: &McpClient) -> String {
    extract_structured(&client.tools_call("host.whoami", json!({})).await.expect("whoami"))["namespace"]
        .as_str()
        .expect("namespace")
        .to_string()
}
