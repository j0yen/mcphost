//! PRD-mcphost-openapi-for-rest-facade
//! AC1 (P0) -- Given `build_router`'s anonymous routes, When the builder pins
//! a `schemars` response schema per route and a test validates one captured
//! response each, Then >= 10 routes pass; the test prints the pinned and
//! unpinnable lists and fails the premise below 10.

use crate::common;
use common::TestServer;
use mcphost::http::routes::{Auth, ROUTES, SchemaRef};

const PREMISE_MIN: usize = 10;

#[tokio::test]
async fn oapi_ac01() {
    let server = TestServer::start().await;
    let http = reqwest::Client::new();
    let mut pinned = Vec::new();
    let mut unpinnable = Vec::new();
    let mut failures = Vec::new();

    for route in ROUTES {
        let ok_schema = route.responses.iter().find_map(|(status, body)| match (status, body) {
            (200, SchemaRef::Json(name)) => Some(*name),
            _ => None,
        });
        let capturable = route.method == "GET" && route.auth == Auth::Anonymous && !route.path.contains('{');
        let (Some(name), true) = (ok_schema, capturable) else {
            unpinnable.push(format!("{} {}", route.method, route.path));
            continue;
        };
        let resp = http
            .get(format!("{}{}", server.base_url, route.path))
            .send()
            .await
            .unwrap_or_else(|e| panic!("GET {}: {e}", route.path));
        if resp.status() != 200 {
            failures.push(format!("{}: expected 200, got {}", route.path, resp.status()));
            continue;
        }
        let body: serde_json::Value = resp
            .json()
            .await
            .unwrap_or_else(|e| panic!("GET {} body is not JSON: {e}", route.path));
        let validator = jsonschema::validator_for(&mcphost::openapi::schema(name)).expect("pinned schema compiles");
        let errors: Vec<String> = validator.iter_errors(&body).map(|e| e.to_string()).collect();
        if errors.is_empty() {
            pinned.push(format!("{} -> {name}", route.path));
        } else {
            failures.push(format!("{} vs {name}: {}", route.path, errors.join("; ")));
        }
    }

    println!("PINNED ({}):\n  {}", pinned.len(), pinned.join("\n  "));
    println!("UNPINNABLE ({}):\n  {}", unpinnable.len(), unpinnable.join("\n  "));
    assert!(failures.is_empty(), "captured responses violate their pinned schema:\n{}", failures.join("\n"));
    assert!(
        pinned.len() >= PREMISE_MIN,
        "premise failed: only {} of the anonymous routes have a pinnable response schema (need {PREMISE_MIN})",
        pinned.len()
    );
}
