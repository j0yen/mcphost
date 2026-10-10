//! PRD-mcphost-unknown-import-domain-hint
//! AC7 (P1) -- Given anonymous `host.quickstart`, When called, Then
//! `sandbox_api.not_a_module` lists `mcphost.dev` and `mcphost_dev`.

use crate::common;
use mcphost::kinds::python::domain_spellings;
use serde_json::json;

#[tokio::test]
async fn anonymous_quickstart_lists_the_domain_spellings_that_are_not_modules() {
    // The anonymous handler arm is `control::quickstart(state, None, args,
    // None)`; the state's public URL is the configured production one (the
    // `TestServer` binds an ephemeral 127.0.0.1 URL instead).
    let (mut state, _dir) = common::bare_app_state().await;
    state.public_url = "https://mcphost.dev".to_string();
    let structured = mcphost::control::quickstart(&state, None, &json!({}), None).expect("anonymous host.quickstart");
    assert_eq!(structured["authenticated"], json!(false));

    let listed: Vec<String> = structured["sandbox_api"]["not_a_module"]
        .as_array()
        .expect("sandbox_api.not_a_module is an array")
        .iter()
        .map(|v| v.as_str().expect("string").to_string())
        .collect();
    assert!(listed.contains(&"mcphost.dev".to_string()), "{listed:?}");
    assert!(listed.contains(&"mcphost_dev".to_string()), "{listed:?}");
    assert_eq!(listed, domain_spellings("mcphost.dev"));
    assert_eq!(structured["sandbox_api"]["import"], json!("import mcphost"));
}
