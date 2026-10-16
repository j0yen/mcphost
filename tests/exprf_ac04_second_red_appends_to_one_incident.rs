//! PRD-mcphost-example-proof-nightly-and-hide
//! AC4 — Given two consecutive failing reports for one name, When the second
//! lands, Then exactly one open incident exists on component `examples` with
//! two entries.

use crate::exprf;

#[tokio::test]
async fn exprf_ac04() {
    let f = exprf::start().await;
    let name = exprf::names()[0];

    exprf::report(&f.admin, name, false).await.expect("first red");
    let first = f.server.state.db.get_example_proof(name.to_string()).await.expect("db").expect("row");
    exprf::report(&f.admin, name, false).await.expect("second red");
    let second = f.server.state.db.get_example_proof(name.to_string()).await.expect("db").expect("row");
    assert_eq!(first.red_since, second.red_since, "red_since keeps the streak's first night");

    let open = f.server.state.db.list_open_incidents().await.expect("open incidents");
    let on_examples: Vec<_> = open.iter().filter(|i| i.components_json.contains("\"examples\"")).collect();
    assert_eq!(on_examples.len(), 1, "exactly one open incident on `examples`: {open:?}");
    assert_eq!(on_examples[0].title, format!("example {name} proof failed"));
    let timeline: Vec<serde_json::Value> = serde_json::from_str(&on_examples[0].timeline_json).expect("timeline json");
    assert_eq!(timeline.len(), 2, "opened + appended: {timeline:?}");
    assert!(timeline[0]["message"].as_str().unwrap_or_default().contains(&format!("line for {name}")), "excerpt on open: {timeline:?}");
}
