//! AC3: an `example_proofs` row hiding `uptime-probes` removes its card, makes
//! its page a 404 with nearest names, and drops it from `/examples.json` for an
//! anonymous reader; an admin bearer with `?include_hidden=1` sees it with its
//! `hidden_reason`.

use crate::common;
use common::{ADMIN_KEY, TestServer};
use serde_json::Value;

fn seed_hidden(server: &TestServer) {
    let conn = rusqlite::Connection::open(server.state.db.path()).unwrap();
    conn.execute_batch(
        "CREATE TABLE example_proofs (name TEXT PRIMARY KEY, ok INTEGER NOT NULL, at TEXT, log_excerpt TEXT, hidden_reason TEXT);
         INSERT INTO example_proofs (name, ok, at, hidden_reason) VALUES ('uptime-probes', 0, '2026-10-09', 'proof failed 2026-10-09');
         INSERT INTO example_proofs (name, ok, at, log_excerpt) VALUES ('docs-qa', 1, '2026-10-09T03:00:00Z', 'line1');",
    )
    .unwrap();
}

fn names(body: &Value) -> Vec<String> {
    body["examples"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap().to_string()).collect()
}

#[tokio::test]
async fn exgal_ac03_hidden_example_is_absent_everywhere() {
    let server = TestServer::start().await;
    seed_hidden(&server);
    let http = reqwest::Client::new();
    let url = |p: &str| format!("{}{p}", server.base_url);

    let page = http.get(url("/examples")).send().await.unwrap().text().await.unwrap();
    assert!(!page.contains("uptime-probes"), "hidden card is absent");
    assert!(page.contains("class=\"badge\">proven 2026-10-09<"), "a passing proof shows a dated badge");

    let detail = http.get(url("/examples/uptime-probes")).send().await.unwrap();
    assert_eq!(detail.status(), 404);
    let detail = detail.text().await.unwrap();
    assert_eq!(detail.matches("<li>").count(), 3, "three nearest names: {detail}");
    assert!(!detail.contains("href=\"/examples/uptime-probes\""));
    assert_eq!(http.get(url("/examples/docs-qa")).send().await.unwrap().status(), 200);

    let anon = http.get(url("/examples.json")).send().await.unwrap();
    assert_eq!(anon.status(), 200);
    assert_eq!(anon.headers()["cache-control"], "max-age=300");
    let anon: Value = anon.json().await.unwrap();
    assert!(!names(&anon).contains(&"uptime-probes".to_string()));
    assert!(names(&anon).contains(&"docs-qa".to_string()));

    // include_hidden without the admin bearer is still the anonymous view.
    let sneaky: Value = http.get(url("/examples.json?include_hidden=1")).send().await.unwrap().json().await.unwrap();
    assert!(!names(&sneaky).contains(&"uptime-probes".to_string()));

    let admin: Value = http
        .get(url("/examples.json?include_hidden=1"))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let row = admin["examples"].as_array().unwrap().iter().find(|e| e["name"] == "uptime-probes").expect("admin sees the hidden row");
    assert_eq!(row["proof"]["hidden_reason"], "proof failed 2026-10-09");
    assert_eq!(row["proof"]["ok"], false);
}
