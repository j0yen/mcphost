//! PRD-mcphost-dry-run-side-effects
//! AC3 — Given a webhook trigger whose tool posts to a channel, When
//! `host.trigger.test` fires it, Then `dry_run.writes` contains
//! `{store: "channel", op: "post", ...}`, `delivered: false`,
//! `host.channel.read` shows no new post, and no agent-wake run row is
//! created.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, extract_structured, publish, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn trigger_test_on_webhook_short_circuits_channel_post_and_reports_dry_run() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns_a, key_a) = signup(&server.base_url, "Dry Run AC3 Owner").await;
    let (ns_b, key_b) = signup(&server.base_url, "Dry Run AC3 Member").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);

    client_a.tools_call("host.group.create", json!({"name": "g"})).await.expect("group.create");
    client_a
        .tools_call("host.group.add", json!({"name": "g", "namespace": ns_b.clone()}))
        .await
        .expect("group.add");
    let opened = extract_structured(
        &client_a
            .tools_call("host.channel.open", json!({"group": "g"}))
            .await
            .expect("channel.open"),
    );
    let channel_id = opened["channel_id"].as_str().expect("channel_id").to_string();

    // B binds a message trigger (agent-wake) to this channel -- AC3's "no
    // agent-wake run row is created" needs a real trigger that COULD fire
    // to be a meaningful assertion.
    client_b
        .tools_call(
            "host.tool_publish",
            json!({"name": "handle_msg", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish handle_msg");
    client_b
        .tools_call(
            "host.trigger.set",
            json!({"tool": "handle_msg", "kind": "message", "channel_id": channel_id}),
        )
        .await
        .expect("trigger.set message");

    let source = "from mcphost import channel\ndef main(args):\n    return channel.post(args[\"body\"][\"cid\"], {\"n\": 1})\n";
    publish(&client_a, "poster", "python", json!({"source": source})).await;

    let set = extract_structured(
        &client_a
            .tools_call(
                "host.trigger.set",
                json!({"tool": "poster", "kind": "webhook", "name": "chanpost"}),
            )
            .await
            .expect("trigger.set webhook"),
    );
    let trigger_id = set["id"].as_str().expect("id").to_string();

    let tested = extract_structured(
        &client_a
            .tools_call(
                "host.trigger.test",
                json!({"id": trigger_id, "body": {"cid": channel_id}}),
            )
            .await
            .expect("trigger.test"),
    );
    let run_id = tested["run_id"].as_str().expect("run_id present").to_string();

    let deadline = Instant::now() + Duration::from_secs(5);
    let run_result = loop {
        let got = extract_structured(
            &client_a
                .tools_call("host.runs.get", json!({"run_id": run_id}))
                .await
                .expect("runs.get"),
        );
        if got["status"] == json!("done") {
            break got["result"].clone();
        }
        assert_ne!(got["status"], json!("error"), "run must not error: {got:?}");
        if Instant::now() >= deadline {
            panic!("run {run_id} never finished: {got:?}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    assert_eq!(
        run_result["dry_run"]["writes"],
        json!([{"store": "channel", "op": "post", "channel": channel_id}]),
        "{run_result:?}"
    );
    assert_eq!(run_result["dry_run"]["delivered"], json!(false), "{run_result:?}");
    assert_eq!(run_result["dry_run"]["rolled_back"], json!(true), "{run_result:?}");
    assert_eq!(run_result["result"]["delivered"], json!(false), "{run_result:?}");

    let read = extract_structured(
        &client_a
            .tools_call("host.channel.read", json!({"channel_id": channel_id}))
            .await
            .expect("host.channel.read"),
    );
    assert_eq!(
        read["posts"].as_array().expect("posts array").len(),
        0,
        "a dry run must not deliver the channel post: {read:?}"
    );

    let runs = extract_structured(
        &client_b
            .tools_call("host.runs.list", json!({"trigger": "message"}))
            .await
            .expect("runs.list"),
    );
    assert_eq!(
        runs["runs"].as_array().expect("runs array").len(),
        0,
        "no agent-wake run must be created: {runs:?}"
    );
}
