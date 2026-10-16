//! PRD-mcphost-openapi-for-rest-facade
//! AC2 (P0) -- Given the built router, When the drift test probes every
//! `ROUTES` entry and walks every answered route, Then the sets are equal
//! modulo `UNSPECCED`; one route added outside the table fails the test
//! naming its path.
//!
//! "Probes" is a `PATCH` to the entry's path (path parameters filled with a
//! dummy): no spec'd route takes `PATCH`, so a mounted path answers `405` with
//! an `Allow` header naming its methods while an unmounted path answers `404`
//! -- no handler runs, so no state changes and no `404 route-not-found` can be
//! mistaken for a handler's own 404. "Walks" is reading every path literal
//! `build_router` hands to `.route(`/`.route_service(` out of `src/http.rs`
//! (axum exposes no route list), then probing each one the same way.

use crate::common;
use common::TestServer;
use mcphost::http::routes::{ROUTES, UNSPECCED};
use std::collections::{BTreeMap, BTreeSet};

fn build_router_source() -> String {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/http.rs")).expect("read src/http.rs");
    let start = src
        .find("pub fn build_router_with_session_mode")
        .expect("build_router_with_session_mode in src/http.rs");
    let end = src[start..]
        .find("/// Serve on an already-bound listener")
        .map(|i| start + i)
        .expect("end of build_router_with_session_mode");
    src[start..end].to_string()
}

/// Every literal path passed straight to `.route(` / `.route_service(` in
/// `source`, comments stripped, in source order.
fn literal_route_paths(source: &str) -> Vec<String> {
    let code: String = source
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut out = Vec::new();
    for needle in [".route(", ".route_service("] {
        let mut rest = code.as_str();
        while let Some(i) = rest.find(needle) {
            rest = &rest[i + needle.len()..];
            let arg = rest.trim_start();
            if let Some(lit) = arg.strip_prefix('"') {
                if let Some(end) = lit.find('"') {
                    out.push(lit[..end].to_string());
                }
            } else if arg.starts_with("crate::oauth::OAUTH_ENTRY_PATH") {
                out.push(mcphost::oauth::OAUTH_ENTRY_PATH.to_string());
            }
        }
    }
    out
}

/// Walked paths that are neither in `ROUTES` nor in `UNSPECCED`.
fn unaccounted(walked: &[String]) -> Vec<String> {
    let known: BTreeSet<&str> = ROUTES.iter().map(|r| r.path).chain(UNSPECCED.iter().copied()).collect();
    walked
        .iter()
        .filter(|p| !known.contains(p.as_str()))
        .cloned()
        .collect()
}

fn fill(path: &str) -> String {
    let mut out = String::new();
    let mut in_param = false;
    for c in path.chars() {
        match c {
            '{' => {
                in_param = true;
                out.push_str("probe");
            }
            '}' => in_param = false,
            _ if !in_param => out.push(c),
            _ => {}
        }
    }
    out
}

async fn probe(base: &str, path: &str) -> (u16, BTreeSet<String>) {
    let resp = reqwest::Client::new()
        .request(reqwest::Method::PATCH, format!("{base}{}", fill(path)))
        .send()
        .await
        .unwrap_or_else(|e| panic!("PATCH {path}: {e}"));
    let allow = resp
        .headers()
        .get("allow")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(',')
        .map(|m| m.trim().to_ascii_uppercase())
        .filter(|m| !m.is_empty())
        .collect();
    (resp.status().as_u16(), allow)
}

#[tokio::test]
async fn oapi_ac02() {
    let server = TestServer::start().await;
    let mut problems = Vec::new();

    // (a) every ROUTES entry resolves, and a path answers exactly its entries' methods.
    let mut methods_by_path: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for r in ROUTES {
        methods_by_path.entry(r.path).or_default().insert(r.method.to_string());
    }
    for (path, methods) in &methods_by_path {
        let (status, allow) = probe(&server.base_url, path).await;
        if status != 405 {
            problems.push(format!("ROUTES path {path} is not mounted in the router (PATCH answered {status}, want 405)"));
            continue;
        }
        let served: BTreeSet<String> = allow.into_iter().filter(|m| m != "HEAD").collect();
        if &served != methods {
            problems.push(format!("{path}: router serves {served:?}, ROUTES says {methods:?}"));
        }
    }

    // (b) every route the router answers is in ROUTES or UNSPECCED.
    let walked = literal_route_paths(&build_router_source());
    assert!(walked.len() >= 8, "source walk found too few routes: {walked:?}");
    problems.extend(unaccounted(&walked).into_iter().map(|p| format!("route {p} is served outside ROUTES/UNSPECCED")));
    // ...and every non-MCP UNSPECCED page is really mounted (the MCP routers sit behind guards that 404 a dummy id).
    for path in UNSPECCED.iter().filter(|p| !p.ends_with("/mcp")) {
        let (status, _) = probe(&server.base_url, path).await;
        if status != 405 {
            problems.push(format!("UNSPECCED path {path} is not mounted (PATCH answered {status}, want 405)"));
        }
    }

    assert!(problems.is_empty(), "route table drift:\n{}", problems.join("\n"));
}

#[test]
fn oapi_ac02_route_added_outside_the_table_is_named() {
    let clean = build_router_source();
    assert_eq!(unaccounted(&literal_route_paths(&clean)), Vec::<String>::new());
    let tampered = clean.replace(
        "Router::new()\n        .merge(table_router)",
        "Router::new()\n        .merge(table_router)\n        .route(\"/zzz-added-outside-the-table\", get(|| async { \"x\" }))",
    );
    assert_ne!(tampered, clean, "the tamper anchor must exist in build_router");
    assert_eq!(unaccounted(&literal_route_paths(&tampered)), vec!["/zzz-added-outside-the-table".to_string()]);
}
