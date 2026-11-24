//! PRD-mcphost-refusals-name-the-nearest-move
//! AC4 (P0) -- Given this PRD's tree, When a source scan lists every
//! `AppError::ToolNotFound(` and `ToolNotFoundOnImplicitTenant` construction in
//! `src/handler.rs` and a matrix test drives the five sites named in R1, Then
//! each construction uses the one builder, each site's refusal carries
//! `nearest`, `did_you_mean`, `tools_published` and `catalog_matches`, and the
//! old `host.tool.list {}` assertions are revised.

use crate::common::{McpClient, RpcError, TestServer, signup};
use serde_json::{Value, json};

const ADMIN_KEY: &str = "nmove-ac04-admin-key";

/// Non-comment lines of `src/handler.rs` that spell either construction.
fn constructions() -> Vec<(usize, String)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let src = std::fs::read_to_string(root.join("src/handler.rs")).expect("read src/handler.rs");
    src.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim_start().starts_with("//"))
        .filter(|(_, l)| l.contains("AppError::ToolNotFound(") || l.contains("ToolNotFoundOnImplicitTenant"))
        .map(|(i, l)| (i + 1, l.trim().to_string()))
        .collect()
}

#[test]
fn nmove_ac04_source_scan_finds_no_construction_outside_the_builder() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let src = std::fs::read_to_string(root.join("src/handler.rs")).expect("read src/handler.rs");
    let found = constructions();
    assert!(found.is_empty(), "a tool_not_found built by hand in src/handler.rs, not by tool_not_found_near: {found:?}");
    let code: Vec<&str> = src.lines().filter(|l| !l.trim_start().starts_with("//")).collect();
    // The builder constructs the one variant; every site calls the builder.
    assert_eq!(code.iter().filter(|l| l.contains("AppError::ToolNotFoundNear {")).count(), 1);
    let calls = code.iter().filter(|l| l.contains(".tool_not_found_near(")).count();
    assert!(calls >= 5, "the five R1 sites call the one builder, found {calls}");
}

fn assert_carries_the_four(site: &str, err: &RpcError) {
    assert_eq!(err.error_code.as_deref(), Some("tool_not_found"), "{site}: {err:?}");
    let d = &err.data;
    assert!(d["nearest"].as_array().is_some_and(|n| !n.is_empty() && n.len() <= 5), "{site}: nearest {d}");
    assert!(d["did_you_mean"].is_array(), "{site}: did_you_mean {d}");
    assert!(d["tools_published"].is_i64(), "{site}: tools_published {d}");
    assert!(d["catalog_matches"].is_i64(), "{site}: catalog_matches {d}");
    assert!(d["next_call"]["tool"].is_string(), "{site}: next_call {d}");
}

#[tokio::test]
async fn nmove_ac04() {
    let server = TestServer::start_with(Some(ADMIN_KEY.to_string())).await;
    let (_ns, key) = signup(&server.base_url, "Nmove04").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let missing = "nmove_ac04_missing";

    let sites: Vec<(&str, &str, Value)> = vec![
        ("dispatch miss", missing, json!({})),
        ("get_tool", "host.tool_run", json!({"name": missing, "args": {}})),
        ("require_tested", "host.tool_share", json!({"name": missing, "visibility": "public"})),
        ("verb-forward miss", "host.tool_test", json!({"name": missing})),
    ];
    for (site, tool, args) in sites {
        let err = client.tools_call(tool, args).await.expect_err(site);
        assert_carries_the_four(site, &err);
        assert_eq!(err.data["tools_published"], 0, "{site}");
        assert_ne!(err.data["next_call"]["tool"], "host.tool.list", "{site}: the old class-level row is gone");
        assert_eq!(err.data["next_call"], json!({"tool": "host.tool_publish", "args": {"name": missing}}), "{site}");
    }

    // The admin path: the admin bearer's unknown `admin.*` name.
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let err = admin.tools_call("admin.nmove_ac04_missing", json!({})).await.expect_err("admin path");
    assert_carries_the_four("admin path", &err);
    assert_eq!(err.data["tools_published"], 0);
    assert_eq!(err.data["next_call"]["tool"], "host.whoami", "an admin cannot publish: {err:?}");
    assert!(
        err.data["nearest"].as_array().unwrap().iter().all(|n| n.as_str().is_some_and(|n| n.starts_with("admin."))),
        "an admin's candidates are admin tools: {err:?}"
    );

    // The same miss with a tool published: the count follows, host.tool.list is still not the move.
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "nmove_ac04_present", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    let err = client.tools_call(missing, json!({})).await.expect_err("still missing");
    assert_eq!(err.data["tools_published"], 1);
    assert_ne!(err.data["next_call"]["tool"], "host.tool.list");
}
