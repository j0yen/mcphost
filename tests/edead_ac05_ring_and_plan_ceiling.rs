//! PRD-mcphost-event-trigger-from-webhook AC5 (P0) — Given
//! `dead_letters_max` 100, When 101 letters are written, Then 100 remain,
//! the oldest is gone, the list reports `dropped_count: 1`, and
//! `host.quickstart` shows `limits.plan.dead_letters_max: 100` for every
//! plan a test renders.

use crate::common;
use common::{extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn hundred_and_first_letter_evicts_the_oldest() {
    let server = common::TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Edead AC5 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "ring_hook", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    client
        .tools_call(
            "host.trigger.set",
            json!({
                "tool": "ring_hook",
                "name": "ring",
                "kind": "event",
                "verify": {"scheme": "none", "allow_unverified": true},
            }),
        )
        .await
        .expect("trigger.set");

    // Free plan: 30 deliveries a minute are accepted, every further one is
    // refused `events_rate_limited` -- 101 letters from deliveries 30..=130.
    let http = reqwest::Client::new();
    let url = format!("{}/hooks/{}/ring_hook", server.base_url, ns);
    for n in 0..131 {
        let resp = http.post(&url).body(json!({"n": n}).to_string()).send().await.expect("POST");
        assert_eq!(resp.status(), if n < 30 { 202 } else { 429 }, "delivery {n}");
    }

    let listed = extract_structured(
        &client
            .tools_call("host.trigger.dead_letters", json!({"trigger": "ring", "limit": 200}))
            .await
            .expect("dead_letters"),
    );
    let letters = listed["dead_letters"].as_array().expect("dead_letters array");
    assert_eq!(letters.len(), 100, "ring holds dead_letters_max letters");
    assert_eq!(listed["dropped_count"], json!(1));
    let bodies: Vec<&str> = letters.iter().map(|l| l["body"].as_str().expect("body")).collect();
    assert_eq!(bodies.first().copied(), Some(json!({"n": 130}).to_string().as_str()), "newest first");
    assert_eq!(bodies.last().copied(), Some(json!({"n": 31}).to_string().as_str()));
    assert!(
        !bodies.contains(&json!({"n": 30}).to_string().as_str()),
        "the oldest letter was evicted"
    );
}

#[tokio::test]
async fn quickstart_and_every_rendered_plan_carry_dead_letters_max() {
    let server = common::TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Edead AC5 Quickstart").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);
    let quickstart = extract_structured(
        &client
            .tools_call("host.quickstart", json!({"kind": "echo"}))
            .await
            .expect("quickstart"),
    );
    assert_eq!(quickstart["limits"]["plan"]["dead_letters_max"], json!(100), "{quickstart}");

    // The same renderer quickstart uses, for every plan of the catalog.
    let catalog = mcphost::plans::PlanCatalog::default_catalog();
    assert!(!catalog.plans.is_empty());
    for plan in &catalog.plans {
        let rendered = plan.ceilings_json();
        assert_eq!(
            rendered["dead_letters_max"],
            json!(plan.dead_letters_max),
            "plan {} must render dead_letters_max",
            plan.name
        );
    }
    let free = catalog.get("free").expect("free plan");
    assert_eq!(free.ceilings_json()["dead_letters_max"], json!(100));
    assert_eq!(catalog.get("pro").expect("pro plan").dead_letters_max, 1000);
}
