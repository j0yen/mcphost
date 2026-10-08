//! AC4 (PRD-mcphost-install-links-more-clients) — Given `/connect/go/goose`
//! from a non-fleet IP, When handled, Then a funnel event `install_link`
//! with `client=goose`, `origin=human` is recorded and the response is a
//! 302 or a rendered instruction page for non-link kinds;
//! `/connect/go/nonsense` returns 404 and records nothing.

use crate::common;
use common::TestServer;

fn no_redirect() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("client")
}

async fn clicks(server: &TestServer) -> Vec<(String, i64)> {
    let mut v = server.state.db.install_link_click_counts_by_client(0).await.expect("click counts");
    v.sort();
    v
}

#[tokio::test]
async fn goose_click_records_a_human_event_and_renders_the_instruction_page() {
    let server = TestServer::start().await;
    let before = clicks(&server).await;

    let response = no_redirect()
        .get(format!("{}/connect/go/goose", server.base_url))
        .send()
        .await
        .expect("GET /connect/go/goose");
    assert_eq!(response.status(), 200, "a non-link kind renders an instruction page");
    let body = response.text().await.expect("body");
    assert!(body.contains("/extension"), "goose instructions must name /extension: {body}");
    assert!(body.contains(&format!("{}/mcp", server.state.public_url.trim_end_matches('/'))), "{body}");

    let after = clicks(&server).await;
    let count = |v: &[(String, i64)]| v.iter().find(|(c, _)| c == "goose").map(|(_, n)| *n).unwrap_or(0);
    assert_eq!(count(&after), count(&before) + 1, "before={before:?} after={after:?}");
}

#[tokio::test]
async fn every_table_id_is_accepted_and_nonsense_is_a_404_that_records_nothing() {
    let server = TestServer::start().await;

    for id in mcphost::install_links::SURFACE_IDS {
        let status = no_redirect()
            .get(format!("{}/connect/go/{id}", server.base_url))
            .send()
            .await
            .expect("GET /connect/go/<id>")
            .status();
        assert!(status == 200 || status == 302, "{id} answered {status}");
    }
    let before = clicks(&server).await;
    assert_eq!(before.len(), mcphost::install_links::SURFACE_IDS.len(), "one human click per id: {before:?}");

    let response = no_redirect()
        .get(format!("{}/connect/go/nonsense", server.base_url))
        .send()
        .await
        .expect("GET /connect/go/nonsense");
    assert_eq!(response.status(), 404);
    assert_eq!(clicks(&server).await, before, "a 404 must record nothing");
}
