//! PRD-mcphost-sync-pull-with-change-detection AC7 -- Given the `SourceKind`
//! registry, When a test calls `fetch` on every kind against its recorded
//! fixture, Then each returns a `Snapshot` with `rows > 0` -- a kind without a
//! source implementation fails to compile against the dispatch `match`.

use crate::common::sypul::Fixture;
use mcphost::sync::SourceKind;
use mcphost::sync::pull::{Cursor, SourceCtx, fetch};

/// The recorded fixture of each kind. No wildcard arm: a kind added to the
/// registry without a fixture here does not compile.
fn recorded(kind: SourceKind) -> (&'static str, &'static str) {
    match kind {
        SourceKind::CsvUrl => (include_str!("fixtures/sypul/orders.csv"), "text/csv"),
        SourceKind::HttpJson => (include_str!("fixtures/sypul/orders.json"), "application/json"),
        SourceKind::GoogleSheet => (include_str!("fixtures/sypul/sheets.json"), "application/json"),
        SourceKind::Airtable => (include_str!("fixtures/sypul/airtable.json"), "application/json"),
        SourceKind::NotionDb => (include_str!("fixtures/sypul/notion.json"), "application/json"),
    }
}

fn reference(kind: SourceKind, fixture: &Fixture) -> String {
    match kind {
        SourceKind::CsvUrl | SourceKind::HttpJson => fixture.url("data"),
        SourceKind::GoogleSheet => "sheet-id/Orders".to_string(),
        SourceKind::Airtable => "appBase/tblOrders".to_string(),
        SourceKind::NotionDb => "database-id".to_string(),
    }
}

#[tokio::test]
async fn sypul_ac07_every_kind_returns_rows_from_its_recorded_fixture() {
    let client = reqwest::Client::new();
    for kind in SourceKind::ALL.iter().copied() {
        let (body, content_type) = recorded(kind);
        let fixture = Fixture::start(body, false).await;
        fixture.set_content_type(content_type);
        let reference = reference(kind, &fixture);
        let ctx = SourceCtx {
            client: &client,
            reference: &reference,
            token: kind.needs_connection().map(|_| "fixture-token"),
            api_base: Some(fixture.base.as_str()),
        };
        let snapshot = fetch(kind, &ctx, &Cursor::default())
            .await
            .unwrap_or_else(|e| panic!("{}: {e}", kind.as_str()));
        assert!(!snapshot.rows.is_empty(), "{}: no rows", kind.as_str());
        assert!(!snapshot.unchanged, "{}: a first fetch is a change", kind.as_str());
        assert!(snapshot.provider_calls >= 1 && snapshot.bytes > 0, "{}: {snapshot:?}", kind.as_str());
        println!("sypul_ac07: {} -> {} rows, first row {}", kind.as_str(), snapshot.rows.len(), snapshot.rows[0].values().map(|v| v.to_string()).collect::<Vec<_>>().join(" | "));

        // The same fetch again, handed back the cursor it earned, is unchanged.
        let cursor = Cursor { etag: snapshot.etag.clone(), modified: snapshot.modified.clone(), body_hash: snapshot.body_hash.clone() };
        let again = fetch(kind, &ctx, &cursor).await.unwrap_or_else(|e| panic!("{}: {e}", kind.as_str()));
        assert!(again.unchanged, "{}: identical body must read as unchanged", kind.as_str());
    }
    // CSV quoting survives: the quoted comma and the doubled quote.
    let fixture = Fixture::start(include_str!("fixtures/sypul/orders.csv"), false).await;
    let ctx = SourceCtx { client: &client, reference: &fixture.url("x.csv"), token: None, api_base: None };
    let csv = fetch(SourceKind::CsvUrl, &ctx, &Cursor::default()).await.expect("csv");
    assert_eq!(csv.rows[0]["name"], "Widget, large");
    assert_eq!(csv.rows[2]["name"], "He said \"hi\"");
}
