//! PRD-mcphost-example-proof-nightly-and-hide
//! AC3 — Given a hidden example, When a later report says `ok: true`, Then
//! `hidden_reason` and `red_since` are null, the incident is resolved, and
//! the example is listed again.

use crate::exprf;

#[tokio::test]
async fn exprf_ac03() {
    let f = exprf::start().await;
    let name = exprf::names()[0];

    exprf::report(&f.admin, name, false).await.expect("red report");
    assert!(!exprf::listed_names(&f.tenant).await.contains(&name.to_string()), "hidden after the red report");
    assert_eq!(f.server.state.db.list_open_incidents().await.expect("open").len(), 1);

    let green = exprf::report(&f.admin, name, true).await.expect("green report");
    assert_eq!(green["ok"], true);
    assert!(green["hidden_reason"].is_null() && green["red_since"].is_null(), "{green}");

    let row = f.server.state.db.get_example_proof(name.to_string()).await.expect("db").expect("row");
    assert!(row.ok && row.hidden_reason.is_none() && row.red_since.is_none(), "{row:?}");
    assert!(f.server.state.db.list_open_incidents().await.expect("open").is_empty(), "incident resolved");
    let recent = f.server.state.db.list_recent_closed_incidents(0).await.expect("closed");
    assert!(recent.iter().any(|i| i.title == format!("example {name} proof failed") && i.closed_at.is_some()), "{recent:?}");
    assert!(exprf::listed_names(&f.tenant).await.contains(&name.to_string()), "listed again");
}
