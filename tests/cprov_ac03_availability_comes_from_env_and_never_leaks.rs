//! PRD-mcphost-connection-provider-registry AC3 (P0): with
//! `MCPHOST_CONNECT_GITHUB_CLIENT_ID` set and
//! `MCPHOST_CONNECT_GITHUB_CLIENT_SECRET` unset, `host.connect.providers`
//! reports `github` as `available: false, reason:
//! "client_secret_not_configured"`, and the response carries neither env
//! value. The host reads the env once at startup into
//! `AppState.connect_providers` (`ConnectProviders::from_env`, which is
//! `from_lookup` over `std::env::var`); the test installs the same snapshot
//! on a real `AppState` from a fixed lookup, so no test mutates process env.

use crate::common;
use mcphost::connect::{ConnectProviders, all_specs, providers_json, providers_tool};
use serde_json::json;

const ID_SENTINEL: &str = "cid-sentinel-ac03-7f3a91";
const OTHER_SECRET_SENTINEL: &str = "gsecret-sentinel-ac03-55b2";

fn lookup(k: &str) -> Option<String> {
    match k {
        "MCPHOST_CONNECT_GITHUB_CLIENT_ID" => Some(ID_SENTINEL.to_string()),
        // Present on another provider, to prove no secret value is returned.
        "MCPHOST_CONNECT_GOOGLE_CLIENT_ID" => Some("gid-sentinel-ac03".to_string()),
        "MCPHOST_CONNECT_GOOGLE_CLIENT_SECRET" => Some(OTHER_SECRET_SENTINEL.to_string()),
        "MCPHOST_CONNECT_SLACK_CLIENT_ID" => Some(String::new()),
        _ => None,
    }
}

#[tokio::test]
async fn github_with_id_but_no_secret_is_unavailable_with_reason_and_leaks_nothing() {
    let (mut state, _dir) = common::bare_app_state().await;
    state.connect_providers = ConnectProviders::from_lookup(all_specs(), lookup);

    let body = providers_tool(&state, &json!({})).expect("host.connect.providers ok");
    let rows = body["providers"].as_array().expect("providers array");
    let github = rows.iter().find(|p| p["name"] == "github").expect("github row");
    assert_eq!(github["available"], false, "{github}");
    assert_eq!(github["reason"], "client_secret_not_configured", "{github}");

    let google = rows.iter().find(|p| p["name"] == "google").expect("google row");
    assert_eq!(google["available"], true, "{google}");
    assert!(google.get("reason").is_none(), "{google}");
    let slack = rows.iter().find(|p| p["name"] == "slack").expect("slack row");
    assert_eq!(slack["reason"], "client_id_not_configured", "an empty value counts as unset: {slack}");

    let raw = body.to_string();
    for sentinel in [ID_SENTINEL, OTHER_SECRET_SENTINEL, "gid-sentinel-ac03"] {
        assert!(!raw.contains(sentinel), "env value '{sentinel}' leaked: {raw}");
    }
}

#[test]
fn rendered_rows_never_carry_env_values() {
    let status = ConnectProviders::from_lookup(all_specs(), lookup);
    let rows = serde_json::to_string(&providers_json(all_specs(), &status)).unwrap();
    assert!(!rows.contains("sentinel"), "{rows}");
}
