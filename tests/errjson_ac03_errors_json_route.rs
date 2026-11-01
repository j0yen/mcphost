//! PRD-mcphost-errors-json-contract
//! AC3 (P0) — Given an anonymous `GET /errors.json`, When parsed, Then
//! `codes` is sorted, every row has the seven fields, `classes` lists exactly
//! the enum's variant names, and `version` equals the crate version.

use crate::common;
use common::TestServer;
use mcphost::error_registry::{ERROR_CODES, ErrorClass};
use serde_json::Value;

const FIELDS: [&str; 7] = ["code", "family", "class", "remedy", "docs_anchor", "retryable", "help_url"];

#[tokio::test]
async fn errjson_ac03_errors_json_is_sorted_complete_and_versioned() {
    let server = TestServer::start().await;
    let resp = reqwest::Client::new()
        .get(format!("{}/errors.json", server.base_url))
        .send()
        .await
        .expect("GET /errors.json");
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers().get("cache-control").and_then(|v| v.to_str().ok()),
        Some("public, max-age=300")
    );
    let body: Value = resp.json().await.expect("/errors.json body must be JSON");

    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));

    // The enum's variant names, derived from serde itself rather than the
    // table the route reads.
    let mut variants: Vec<String> = ErrorClass::ALL
        .iter()
        .map(|c| serde_json::to_value(c).unwrap().as_str().unwrap().to_string())
        .collect();
    let listed: Vec<String> = body["classes"]
        .as_array()
        .expect("classes array")
        .iter()
        .map(|c| c.as_str().unwrap().to_string())
        .collect();
    assert_eq!(listed.len(), 5, "{listed:?}");
    variants.sort();
    let mut sorted_listed = listed.clone();
    sorted_listed.sort();
    assert_eq!(sorted_listed, variants);

    let codes = body["codes"].as_array().expect("codes array");
    assert_eq!(codes.len(), ERROR_CODES.len());
    let names: Vec<&str> = codes.iter().map(|r| r["code"].as_str().unwrap()).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted, "codes must be sorted by code");
    for row in codes {
        let obj = row.as_object().expect("row is an object");
        for field in FIELDS {
            assert!(obj.contains_key(field), "{row} lacks {field}");
        }
        // `data_keys` (spkind AC5) and `reasons` (ipsess AC4) are the optional extra fields.
        let extra = usize::from(obj.contains_key("data_keys")) + usize::from(obj.contains_key("reasons"));
        assert_eq!(obj.len(), FIELDS.len() + extra, "{row} has fields beyond the seven");
        assert!(listed.contains(&row["class"].as_str().unwrap().to_string()), "{row}");
        assert!(row["retryable"].is_boolean(), "{row}");
        assert!(row["help_url"].as_str().unwrap().ends_with(&format!("/help/{}", row["code"].as_str().unwrap())), "{row}");
    }
}

#[tokio::test]
async fn errjson_ac03_rows_match_the_registry() {
    let server = TestServer::start().await;
    let body: Value = reqwest::Client::new()
        .get(format!("{}/errors.json", server.base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let row = body["codes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["code"] == "tool_exception")
        .expect("tool_exception is listed");
    let reg = mcphost::error_registry::lookup("tool_exception").unwrap();
    assert_eq!(row["class"], reg.class.as_str());
    assert_eq!(row["family"], reg.family.as_str());
    assert_eq!(row["remedy"], reg.remedy);
    assert_eq!(row["docs_anchor"], reg.docs_anchor.as_str());
}
