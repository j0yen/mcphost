//! AC3 (PRD-mcphost-install-links-more-clients) — Given an unauthenticated
//! GET `/connect`, When rendered, Then every surface id appears once,
//! grouped under headings per kind, and the page contains no `<script>`.

use crate::common;
use common::TestServer;
use mcphost::install_links::{self, Kind};

#[tokio::test]
async fn connect_lists_every_surface_once_under_its_kind_heading_without_script() {
    let server = TestServer::start().await;
    let table = install_links::for_url(&server.state.public_url);

    let response = reqwest::Client::new()
        .get(format!("{}/connect", server.base_url))
        .send()
        .await
        .expect("GET /connect");
    assert_eq!(response.status(), 200);
    let body = response.text().await.expect("/connect body");

    assert_eq!(table.len(), 12);
    for s in &table {
        assert_eq!(
            body.matches(&format!("id=\"{}\"", s.id)).count(),
            1,
            "surface {} must appear exactly once: {body}",
            s.id
        );
    }

    // Each kind has one heading, and every surface of that kind sits
    // between its heading and the next one.
    let heading_pos: Vec<(Kind, usize)> = Kind::ALL
        .iter()
        .map(|k| {
            let needle = format!("<h2>{}</h2>", k.heading());
            assert_eq!(body.matches(&needle).count(), 1, "heading {needle} once: {body}");
            (*k, body.find(&needle).expect("heading present"))
        })
        .collect();
    for s in &table {
        let at = body.find(&format!("id=\"{}\"", s.id)).expect("surface present");
        let owning = heading_pos
            .iter()
            .filter(|(_, pos)| *pos < at)
            .max_by_key(|(_, pos)| *pos)
            .map(|(k, _)| *k)
            .expect("a heading precedes every surface");
        assert_eq!(owning, s.kind, "surface {} grouped under the wrong heading", s.id);
    }

    assert!(!body.to_lowercase().contains("<script"), "/connect must render without JavaScript: {body}");
}
