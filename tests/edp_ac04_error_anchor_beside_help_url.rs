//! PRD-mcphost-emitted-doc-pointers-resolve
//! AC4 (P0) — Given a `rate_limited` error with a base URL, When
//! `into_error_data` runs, Then `data.anchor` equals the help entry's
//! `doc_link.as_str()` and `help_url` is unchanged from before this PRD.

use mcphost::errors::AppError;
use mcphost::help;
use serde_json::json;

fn rate_limited() -> AppError {
    AppError::Structured {
        code: "rate_limited",
        message: "too many signups from this source".to_string(),
        data: json!({"retry_after_s": 30}),
    }
}

#[test]
fn edp_ac04_rate_limited_carries_the_help_entrys_anchor_and_the_same_help_url() {
    let base = "https://mcphost.dev";
    let data = rate_limited().into_error_data_at(Some(base)).data.expect("error data");
    let entry = help::entry("rate_limited").expect("rate_limited has a help entry");
    assert_eq!(data["anchor"], entry.doc_link.as_str());
    assert_eq!(data["help_url"], "https://mcphost.dev/help/rate_limited", "help_url is unchanged");
    assert_eq!(data["help_url"].as_str(), help::help_url(base, "rate_limited").as_deref());
    assert_eq!(data["retry_after_s"], 30);
}

#[test]
fn edp_ac04_every_help_code_with_a_help_url_names_an_anchor_host_help_get_accepts() {
    for code in help::HELP_CODES {
        let entry = help::entry(code).unwrap_or_else(|| panic!("{code} has a help entry"));
        assert!(
            mcphost::doc_anchors::DocAnchor::parse(entry.doc_link.as_str()).is_some(),
            "{code}: {} is not a heading",
            entry.doc_link.as_str()
        );
    }
}

#[test]
fn edp_ac04_no_base_url_means_no_help_url_and_no_anchor() {
    let data = rate_limited().into_error_data().data.expect("error data");
    assert!(data.get("help_url").is_none(), "{data}");
    assert!(data.get("anchor").is_none(), "{data}");
}
