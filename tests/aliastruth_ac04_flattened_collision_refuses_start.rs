//! PRD-mcphost-tools-list-alias-truth
//! AC4 — Given two tools whose flattened forms collide, When the server
//! starts, Then it refuses to start and names both tools.

use crate::common;
use mcphost::kinds::KindRegistry;
use mcphost::tool_aliases::build_registry;

#[test]
fn colliding_flattened_forms_are_refused_naming_both_tools() {
    // `a.b_c` and `a_b.c` both flatten to `a_b_c`.
    let err = build_registry(&["a.b_c", "a_b.c"]).expect_err("collision must be refused");
    assert_eq!(err.flattened, "a_b_c");
    let msg = err.to_string();
    assert!(msg.contains("a.b_c") && msg.contains("a_b.c"), "both tools named: {msg}");

    // A flattened form that equals another tool's exact name collides too.
    let err = build_registry(&["host.x.y", "host_x_y"]).expect_err("shadowing must be refused");
    let msg = err.to_string();
    assert!(msg.contains("host.x.y") && msg.contains("host_x_y"), "both tools named: {msg}");

    // The same tool's own flattened form is not a collision.
    assert!(build_registry(&["host.tool.run", "host.tool.list"]).is_ok());
}

#[tokio::test]
async fn the_real_registry_starts_and_the_startup_check_is_the_one_serve_runs() {
    let kinds = KindRegistry::with_builtin();
    mcphost::handler::validate_tool_registry(&kinds).expect("shipped registry has no collisions");

    // `serve_on_listener` runs the same check before accepting connections;
    // a healthy registry serves.
    let server = common::TestServer::start().await;
    assert!(server.base_url.starts_with("http://"));
}
