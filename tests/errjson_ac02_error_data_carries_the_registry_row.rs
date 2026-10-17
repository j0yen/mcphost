//! PRD-mcphost-errors-json-contract
//! AC2 (P0) — Given an error of any registry code raised through
//! `into_error_data`, When the client reads `data`, Then it carries `class`,
//! `family`, `remedy` and `help_url` equal to the registry row, including
//! `tool_exception`, `upstream_status` and `budget_ceiling_exceeded`.

use crate::common;
use common::TestServer;
use mcphost::error_registry::{ERROR_CODES, lookup};
use mcphost::errors::AppError;
use serde_json::{Value, json};

const BASE: &str = "https://mcphost.example/";

fn structured(code: &'static str) -> AppError {
    AppError::Structured { code, message: format!("{code} raised"), data: json!({}) }
}

fn assert_row(err: AppError) {
    let code = err.code();
    let row = lookup(code).unwrap_or_else(|| panic!("{code} has no registry row"));
    let data: Value = err.into_error_data_at(Some(BASE)).data.expect("error data");
    assert_eq!(data["error_code"], code, "{data}");
    assert_eq!(data["class"], row.class.as_str(), "{code}: {data}");
    assert_eq!(data["family"], row.family.as_str(), "{code}: {data}");
    assert_eq!(data["remedy"], row.remedy, "{code}: {data}");
    assert_eq!(data["help_url"], format!("https://mcphost.example/help/{code}"), "{code}: {data}");
}

#[test]
fn errjson_ac02_every_registry_code_carries_class_family_remedy_and_help_url() {
    for row in ERROR_CODES {
        assert_row(structured(row.code));
    }
}

#[test]
fn errjson_ac02_the_named_codes_carry_their_row() {
    for code in ["tool_exception", "upstream_status", "budget_ceiling_exceeded"] {
        assert_row(structured(code));
    }
}

#[test]
fn errjson_ac02_every_app_error_variant_carries_its_row() {
    for variant in mcphost::errors::all_variants_for_test() {
        assert_row(variant);
    }
}

#[test]
fn errjson_ac02_no_base_url_means_no_help_url_but_class_still_rides() {
    let data = structured("tool_exception").into_error_data().data.expect("error data");
    assert!(data.get("help_url").is_none(), "{data}");
    assert_eq!(data["class"], "agent_fault", "{data}");
}

#[tokio::test]
async fn errjson_ac02_help_url_of_a_registry_only_code_resolves() {
    // `tool_exception` has no hand-written HELP_ENTRIES row; its help_url must
    // not 404 (it did on prod before the registry).
    let server = TestServer::start().await;
    let row = lookup("tool_exception").unwrap();
    let resp = reqwest::Client::new()
        .get(format!("{}/help/tool_exception", server.base_url))
        .send()
        .await
        .expect("GET /help/tool_exception");
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("tool_exception"), "{body}");
    let escaped = row.remedy.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    assert!(body.contains(&escaped), "page shows the registry remedy: {body}");

    let missing = reqwest::Client::new()
        .get(format!("{}/help/not_a_code_anywhere", server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);
}
