//! AC2: with the embedded index and no `example_proofs` table, `GET /examples`
//! is 200 with one card per example, every badge `unproven`, and every card's
//! snippet names `host.example.fork` with the right name.

use crate::common;

use common::TestServer;
use mcphost::examples::EXAMPLES;

#[tokio::test]
async fn exgal_ac02_examples_page_has_one_unproven_card_per_example() {
    let server = TestServer::start().await;
    let resp = reqwest::get(format!("{}/examples", server.base_url)).await.unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.headers()["content-type"].to_str().unwrap().starts_with("text/html"));
    let html = resp.text().await.unwrap();

    assert!(!EXAMPLES.is_empty());
    assert_eq!(html.matches("class=\"card\"").count(), EXAMPLES.len(), "one card per example");
    assert_eq!(html.matches("class=\"badge\">unproven<").count(), EXAMPLES.len(), "every badge is unproven");
    assert!(!html.contains("proven 2"), "no dated badge without a proofs table");
    for ex in EXAMPLES {
        let snippet = format!("host.example.fork {{&quot;name&quot;: &quot;{}&quot;}}", ex.name);
        assert!(html.contains(&snippet), "card for {} carries `{snippet}`", ex.name);
    }
}
