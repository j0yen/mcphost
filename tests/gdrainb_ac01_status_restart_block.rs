//! PRD-mcphost-graceful-drain-contract-b AC1 -- Given a host that has never
//! restarted, When `GET /status.json`, Then `restart.last` is `null`,
//! `restart.count_30d` is 0, and every pre-existing key is unchanged; Given
//! one restart, Then `restart.last` has the seven fields of R2.

use crate::common::TestServer;

async fn status(base: &str) -> serde_json::Value {
    reqwest::Client::new()
        .get(format!("{base}/status.json"))
        .send()
        .await
        .expect("GET /status.json")
        .json()
        .await
        .expect("parse /status.json")
}

#[tokio::test]
async fn gdrainb_ac01() {
    let server = TestServer::start().await;

    let mut body = status(&server.base_url).await;
    assert!(body["restart"]["last"].is_null(), "never restarted: {body}");
    assert_eq!(body["restart"]["count_30d"], 0, "{body}");
    let obj = body.as_object_mut().unwrap();
    obj.remove("restart");
    for key in [
        "state",
        "generated_at",
        "components",
        "incidents_open",
        "incidents_recent_30d",
        "warnings",
        "banner",
        "signals",
        "heal",
        "maintenance",
        "feedback",
        "onboarding_7d",
        "docs_gaps",
        "deploy",
        "examples",
    ] {
        assert!(obj.contains_key(key), "pre-existing key {key} missing: {body}");
    }

    server
        .state
        .db
        .record_restart(mcphost::db::RestartRow {
            restart_id: "01TESTRESTART".into(),
            signal_at_unix: mcphost::state::now_unix(),
            in_flight_at_signal: 3,
            finished: 2,
            dropped: 1,
            drain_ms: 1234,
            bound_s: 20,
            version: "9.9.9".into(),
        })
        .await
        .expect("record restart");

    let body = status(&server.base_url).await;
    assert_eq!(body["restart"]["count_30d"], 1, "{body}");
    let last = body["restart"]["last"].as_object().expect("restart.last object");
    let mut keys: Vec<&str> = last.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["at", "bound_s", "drain_ms", "dropped", "finished", "in_flight_at_signal", "version"]);
    assert_eq!(last["dropped"], 1);
    assert_eq!(last["version"], "9.9.9");
}
