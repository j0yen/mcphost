//! PRD-mcphost-elicitation-in-band-on-stateless-transport
//! AC7 (P0) — Given 8 concurrent sessions eliciting at once and answering in
//! shuffled order, When all complete, Then each call receives only its own
//! session's `content`; Given two concurrent eliciting `tools/call`s on ONE
//! session, Then the first gets its own `content` and the second returns
//! `pending`, with no cross-delivery; Given `initialize` floods beyond the
//! capability store's ceiling, Then the store never exceeds it, the oldest
//! entries are evicted and the newest still elicit in-band.

use crate::inband_client;

use crate::common;
use common::{McpClient, TestServer, extract_structured, python_kind_registry, signup};
use inband_client::{RawClient, silent};
use mcphost::sandbox;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const TOOL: &str = "import mcphost\ndef main(args):\n    if args.get(\"warm\"):\n        return {\"warm\": True}\n    return mcphost.elicit(\"Who?\", {\"who\": {\"type\": \"integer\"}}, timeout_s=args.get(\"timeout_s\"))\n";
const FORM: fn() -> Value = || json!({"elicitation": {"form": {}}});

async fn fixture(label: &str) -> (TestServer, common::TempDataDir, String, String) {
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, label).await;
    let admin = McpClient::with_bearer(&server.base_url, &key);
    admin
        .tools_call("host.tool_publish", json!({"name": "who", "kind": "python", "spec": {"source": TOOL}}))
        .await
        .expect("publish");
    (server, envs_dir, format!("{ns}.who"), key)
}

async fn warm(client: &RawClient, name: &str) {
    for _ in 0..200 {
        let out = client.call(name, json!({"warm": true}), &silent).await;
        if out.response["result"]["structuredContent"]["warm"] == json!(true) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("tool never became ready");
}

/// Answers `{who: n}` after `delay_ms`.
fn answer_who(n: u64, delay_ms: u64) -> impl Fn(&Value) -> inband_client::Answer + Sync {
    move |_: &Value| Some((Duration::from_millis(delay_ms), json!({"action": "accept", "content": {"who": n}})))
}

#[tokio::test]
async fn eight_concurrent_sessions_each_get_only_their_own_content() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    // One tenant per session: the free plan caps one tenant at 4 concurrent calls.
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_full_with_signup_rate_limit(
        Some(common::ADMIN_KEY.to_string()),
        python_kind_registry(&envs_dir.0),
        mcphost::state::CALL_TIMEOUT,
        None,
        100,
    )
    .await;
    let mut clients = Vec::new();
    let mut names = Vec::new();
    for n in 0..8 {
        let (ns, key) = signup(&server.base_url, &format!("Elib AC7 eight {n}")).await;
        McpClient::with_bearer(&server.base_url, &key)
            .tools_call("host.tool_publish", json!({"name": "who", "kind": "python", "spec": {"source": TOOL}}))
            .await
            .expect("publish");
        let name = format!("{ns}.who");
        let client = Arc::new(RawClient::open(&server.base_url, &key, FORM(), true).await);
        warm(&client, &name).await;
        clients.push(client);
        names.push(name);
    }
    // Shuffled: session n answers after DELAYS_MS[n], not in call order.
    const DELAYS_MS: [u64; 8] = [700, 100, 600, 200, 500, 300, 400, 0];
    let mut calls = Vec::new();
    for (n, client) in clients.iter().enumerate() {
        let (client, name) = (client.clone(), names[n].clone());
        calls.push(tokio::spawn(async move {
            let n = n as u64;
            client.call(&name, json!({"timeout_s": 20}), &answer_who(n, DELAYS_MS[n as usize])).await
        }));
    }
    for (n, call) in calls.into_iter().enumerate() {
        let out = call.await.expect("join");
        assert_eq!(out.frames.len(), 1, "session {n}: {:?} {}", out.frames, out.response);
        assert_eq!(
            extract_structured(&out.response["result"]),
            json!({"action": "accept", "content": {"who": n}}),
            "session {n} must get only its own content: {}",
            out.response
        );
    }
}

#[tokio::test]
async fn a_second_concurrent_elicit_on_one_session_returns_pending_without_cross_delivery() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let (server, _envs, name, key) = fixture("Elib AC7 one").await;
    let client = Arc::new(RawClient::open(&server.base_url, &key, FORM(), true).await);
    warm(&client, &name).await;

    let asked: Arc<Mutex<bool>> = Arc::default();
    let asked_in = asked.clone();
    let first = {
        let (client, name) = (client.clone(), name.clone());
        tokio::spawn(async move {
            client
                .call(&name, json!({"timeout_s": 20}), &move |_: &Value| {
                    *asked_in.lock().unwrap() = true;
                    Some((Duration::from_secs(3), json!({"action": "accept", "content": {"who": 1}})))
                })
                .await
        })
    };
    while !*asked.lock().unwrap() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // The first holds the session's one in-band slot: this one takes the link path.
    let second = client.call(&name, json!({"timeout_s": 1}), &answer_who(2, 0)).await;
    assert!(second.frames.is_empty(), "no in-band frame for the second: {:?}", second.frames);
    assert_eq!(second.content_type, "application/json");
    let pending = extract_structured(&second.response["result"]);
    assert_eq!(pending["action"], json!("pending"), "{pending}");

    let first = first.await.expect("join");
    assert_eq!(first.frames.len(), 1);
    assert_eq!(
        extract_structured(&first.response["result"]),
        json!({"action": "accept", "content": {"who": 1}}),
        "{}",
        first.response
    );
}

#[tokio::test]
async fn an_initialize_flood_never_exceeds_the_ceiling_and_the_newest_still_elicit_in_band() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let (server, _envs, name, key) = fixture("Elib AC7 flood").await;
    let stores = server.state.session_bindings.inband().clone();
    const CEILING: usize = 8;
    stores.set_capability_ceiling(CEILING);
    let mut clients = Vec::new();
    for _ in 0..(CEILING * 3) {
        clients.push(RawClient::open(&server.base_url, &key, FORM(), true).await);
        assert!(stores.capability_len() <= CEILING, "the store grew to {}", stores.capability_len());
    }
    assert_eq!(stores.capability_len(), CEILING);

    // The oldest entries were evicted: that client is back on the link path.
    let oldest = &clients[0];
    warm(oldest, &name).await;
    let out = oldest.call(&name, json!({"timeout_s": 1}), &answer_who(0, 0)).await;
    assert!(out.frames.is_empty(), "an evicted session sees no frame: {:?}", out.frames);
    assert_eq!(extract_structured(&out.response["result"])["action"], json!("pending"), "{}", out.response);

    // The newest still elicit in-band.
    let newest = clients.last().expect("clients");
    warm(newest, &name).await;
    let out = newest.call(&name, json!({"timeout_s": 20}), &answer_who(7, 0)).await;
    assert_eq!(out.frames.len(), 1, "{:?}", out.frames);
    assert_eq!(
        extract_structured(&out.response["result"]),
        json!({"action": "accept", "content": {"who": 7}}),
        "{}",
        out.response
    );
}
