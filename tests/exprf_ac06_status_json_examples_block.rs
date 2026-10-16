//! PRD-mcphost-example-proof-nightly-and-hide
//! AC6 — Given `/status.json`, When fetched after one failing and six
//! passing reports, Then `examples` reads `{total, visible, hidden:
//! [{name, red_since}], last_run}` with total = the embedded count (7 in the
//! PRD's wording; the index has grown, so the test derives it).

use crate::exprf;

use serde_json::Value;

async fn status_examples(base_url: &str) -> Value {
    let body: Value = reqwest::Client::new()
        .get(format!("{base_url}/status.json"))
        .send()
        .await
        .expect("GET /status.json")
        .json()
        .await
        .expect("status.json is JSON");
    body["examples"].clone()
}

#[tokio::test]
async fn exprf_ac06() {
    let f = exprf::start().await;
    let names = exprf::names();
    let total = names.len();
    assert!(total >= 7, "the PRD's seven-report scenario needs at least seven examples");

    let before = status_examples(&f.server.base_url).await;
    assert_eq!(before["total"], total);
    assert_eq!(before["visible"], total, "no reports: everything visible");
    assert_eq!(before["hidden"], serde_json::json!([]));
    assert!(before["last_run"].is_null(), "{before}");

    // One failing and six passing reports.
    exprf::report(&f.admin, names[0], false).await.expect("red");
    for name in &names[1..7] {
        exprf::report(&f.admin, name, true).await.expect("green");
    }
    let red_since = f.server.state.db.get_example_proof(names[0].to_string()).await.expect("db").expect("row").red_since;

    let after = status_examples(&f.server.base_url).await;
    assert_eq!(after["total"], total);
    assert_eq!(after["visible"], total - 1, "{after}");
    assert_eq!(after["hidden"], serde_json::json!([{"name": names[0], "red_since": red_since}]));
    assert!(after["last_run"].as_i64().is_some_and(|t| t > 0), "{after}");
    let obj = after.as_object().expect("object");
    assert_eq!(obj.len(), 4, "exactly total, visible, hidden, last_run: {after}");
}
