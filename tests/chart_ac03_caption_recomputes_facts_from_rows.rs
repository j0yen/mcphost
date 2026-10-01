//! PRD-mcphost-chart-in-a-minute
//! AC3 — Given the same result (`SELECT category, SUM(amount) AS total FROM
//! expenses GROUP BY category`), When the caption is read, Then its
//! headline names the fixture's largest category and every fact's value
//! equals a value recomputed from the rows in the test.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};

use crate::chart_fixture;

#[tokio::test]
async fn caption_headline_names_largest_category_and_facts_match_recomputed_values() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Chart AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    chart_fixture::seed_expenses(&client).await;

    let result = client
        .tools_call(
            "host.table.chart",
            serde_json::json!({"sql": "SELECT category, SUM(amount) AS total FROM expenses GROUP BY category"}),
        )
        .await
        .expect("host.table.chart");
    let chart = extract_structured(&result);

    // The test's own independent recomputation from the fixture rows --
    // never trusting the tool's own arithmetic.
    let totals = chart_fixture::expected_category_totals();
    let (largest_category, largest_total) = totals
        .iter()
        .cloned()
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
        .expect("at least one category");
    let grand_total: f64 = totals.iter().map(|(_, v)| v).sum();
    let count = totals.len() as i64;

    let headline = chart["caption"]["headline"].as_str().expect("caption.headline is a string");
    assert!(
        headline.contains(largest_category),
        "headline must name the largest category {largest_category}: {headline}"
    );

    let facts = chart["caption"]["facts"].as_array().expect("caption.facts array");
    assert_eq!(facts.len(), 3, "facts: {facts:?}");

    let fact_value = |i: usize| facts[i]["value"].as_f64().expect("fact value is numeric");
    assert!(
        (fact_value(0) - largest_total).abs() < 1e-6,
        "fact[0] (largest) must equal the recomputed max {largest_total}: {facts:?}"
    );
    assert!(
        (fact_value(1) - grand_total).abs() < 1e-6,
        "fact[1] (total) must equal the recomputed sum {grand_total}: {facts:?}"
    );
    assert_eq!(
        facts[2]["value"].as_i64(),
        Some(count),
        "fact[2] (count) must equal the recomputed row count {count}: {facts:?}"
    );

    // Every fact carries a non-empty provenance string (technical
    // considerations: "Facts carry the vendored crate's provenance
    // entries; the tool passes them through unchanged").
    for fact in facts {
        assert!(
            fact["provenance"].as_str().is_some_and(|p| !p.is_empty()),
            "fact missing provenance: {fact:?}"
        );
    }
}
