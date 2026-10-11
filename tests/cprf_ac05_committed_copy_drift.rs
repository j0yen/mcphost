//! PRD-mcphost-client-profile-registry AC5 — Given `www/client-profiles.json`
//! and `docs/client-profiles.md` on disk, When the committed-copy test runs,
//! Then both equal the generator's output byte for byte, and
//! `GET /client-profiles.json` equals the file.
//!
//! The JSON records `generated_from` (the crate version). A release bumps the
//! version after a PRD lands, so the committed copy is compared against the
//! generator run with the version the copy itself records; the served route
//! is compared against the generator run with the running version, and
//! against the file with `generated_from` the only permitted difference.

use crate::common;
use common::TestServer;
use mcphost::client_profiles::{render_json, render_markdown};
use serde_json::Value;

const COMMITTED_JSON: &str = include_str!("../www/client-profiles.json");
const COMMITTED_DOC: &str = include_str!("../docs/client-profiles.md");

fn without_generated_from(s: &str) -> Value {
    let mut v: Value = serde_json::from_str(s).expect("json");
    v.as_object_mut().expect("object").remove("generated_from");
    v
}

#[test]
fn committed_json_equals_generator_output_byte_for_byte() {
    let v: Value = serde_json::from_str(COMMITTED_JSON).expect("json");
    let version = v["generated_from"].as_str().expect("generated_from");
    assert_eq!(
        COMMITTED_JSON,
        render_json(version),
        "www/client-profiles.json has drifted -- run `mcphost gen-docs`"
    );
    assert_eq!(v["profiles"].as_array().map(Vec::len), Some(13));
}

#[test]
fn committed_markdown_equals_generator_output_byte_for_byte() {
    assert_eq!(
        COMMITTED_DOC,
        render_markdown(),
        "docs/client-profiles.md has drifted -- run `mcphost gen-docs`"
    );
}

#[test]
fn gen_docs_check_covers_both_files_and_is_not_stale() {
    let stale = mcphost::gendocs::run(true, &mcphost::kinds::KindRegistry::with_builtin()).expect("gen-docs --check");
    assert!(!stale, "mcphost gen-docs --check reports stale docs -- run `mcphost gen-docs`");
}

#[tokio::test]
async fn served_json_equals_the_file() {
    let server = TestServer::start().await;
    let resp = reqwest::get(format!("{}/client-profiles.json", server.base_url)).await.expect("GET");
    assert_eq!(resp.status(), 200);
    assert!(resp.headers().contains_key("cache-control"));
    let body = resp.text().await.expect("body");
    assert_eq!(body, render_json(env!("CARGO_PKG_VERSION")));
    assert_eq!(without_generated_from(&body), without_generated_from(COMMITTED_JSON));
}
