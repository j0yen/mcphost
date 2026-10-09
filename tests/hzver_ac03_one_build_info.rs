//! PRD-mcphost-healthz-version-field AC3 — Given a running server, When
//! anonymous `/healthz`, admin `/healthz` and `/status.json` are fetched,
//! Then all three `version` and `git_sha` values are byte-equal and equal
//! `build_info::BUILD`.

use crate::common;
use common::{ADMIN_KEY, TestServer};
use mcphost::build_info::BUILD;
use serde_json::{Value, json};

async fn get(base: &str, path: &str, bearer: Option<&str>) -> Value {
    let mut req = reqwest::Client::new().get(format!("{base}{path}"));
    if let Some(b) = bearer {
        req = req.bearer_auth(b);
    }
    req.send().await.unwrap().json().await.unwrap()
}

#[tokio::test]
async fn three_bodies_agree_with_build_info() {
    let server = TestServer::start().await;
    let anon = get(&server.base_url, "/healthz", None).await;
    let admin = get(&server.base_url, "/healthz", Some(ADMIN_KEY)).await;
    let feed = get(&server.base_url, "/status.json", None).await;

    for (name, body) in [("anon", &anon), ("admin", &admin), ("status", &feed)] {
        assert_eq!(body["version"], json!(BUILD.version), "{name}: {body}");
        assert_eq!(body["git_sha"], json!(BUILD.git_sha), "{name}: {body}");
    }
    assert_eq!(BUILD.version, env!("CARGO_PKG_VERSION"));
    // Byte-equal renderings, not merely equal after parsing.
    let render = |b: &Value| format!("{}{}", b["version"], b["git_sha"]);
    assert_eq!(render(&anon), render(&admin));
    assert_eq!(render(&anon), render(&feed));
}
