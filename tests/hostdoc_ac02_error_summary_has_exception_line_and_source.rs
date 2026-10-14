//! PRD-mcphost-host-doctor-tool
//! AC2 -- Given a python tool that raises at `tool.py:16`, When the call
//! fails, Then its `calls.error_summary` is `tool_exception: TypeError: string
//! indices must be integers, not 'str' at tool.py:16` followed by the source
//! line, <= 512 bytes, and contains no argument value.

use crate::ctrace;

use mcphost::sandbox;
use serde_json::json;

/// A tool whose `rows["name"]` is on line 16 of `tool.py`.
fn source() -> String {
    let lines = [
        "# a tool that indexes a string as if it were a dict",      // 1
        "",                                                         // 2
        "",                                                         // 3
        "",                                                         // 4
        "",                                                         // 5
        "",                                                         // 6
        "",                                                         // 7
        "",                                                         // 8
        "def main(args):",                                          // 9
        "    rows = 'not a dict'",                                  // 10
        "    seen = args.get('note')",                              // 11
        "    print(seen)",                                          // 12
        "",                                                         // 13
        "",                                                         // 14
        "",                                                         // 15
        "    return rows[\"name\"]",                                // 16
    ];
    lines.join("\n") + "\n"
}

#[tokio::test]
async fn failed_python_call_stores_exception_line_and_source_without_arguments() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let fx = ctrace::start().await;
    fx.publish_python("indexer", &source()).await;
    fx.warm("indexer", json!({})).await;
    let tenant_id = fx.tenant_id().await;

    let secret_arg = "ARGVALUE-7f3a91c2";
    let err = fx
        .tool_call("indexer", json!({"note": secret_arg}))
        .await
        .expect_err("the tool raises");
    assert_eq!(err.error_code.as_deref(), Some("tool_exception"), "{} {}", err.code, err.message);

    let conn = rusqlite::Connection::open(fx.server.state.db.path()).expect("open the calls db");
    let summary: String = conn
        .query_row(
            "SELECT error_summary FROM calls WHERE tenant_id = ?1 AND tool_name = 'indexer' AND ok = 0 \
             ORDER BY id DESC LIMIT 1",
            [tenant_id],
            |r| r.get(0),
        )
        .expect("a failed call row with an error_summary");
    assert_eq!(
        summary,
        "tool_exception: TypeError: string indices must be integers, not 'str' at tool.py:16\nreturn rows[\"name\"]",
        "{summary:?}"
    );
    assert!(summary.len() <= 512, "{} bytes", summary.len());
    assert!(!summary.contains(secret_arg), "argument value leaked: {summary:?}");

    // The same call, read back through host.doctor, carries the line too.
    let doctor = fx
        .client
        .tools_call("host.doctor", json!({}))
        .await
        .map(|v| crate::common::extract_structured(&v))
        .expect("host.doctor");
    let group = &doctor["groups"][0];
    assert_eq!(group["line"], json!(16), "{doctor}");
    assert_eq!(group["first_source_line"], json!("return rows[\"name\"]"), "{doctor}");
}

#[test]
fn summary_is_capped_and_never_carries_a_credential() {
    let message = format!("ValueError: tenant_key=t_abcdef0123456789 Bearer sk-live-9999 {}", "x".repeat(900));
    let data = json!({"line": 3, "source_line": "token = fetch(secret=SUPERSECRET)"});
    let summary = mcphost::doctor::error_summary("tool_exception", &message, &data);
    assert!(summary.len() <= 512, "{} bytes", summary.len());
    for leaked in ["t_abcdef0123456789", "sk-live-9999"] {
        assert!(!summary.contains(leaked), "{leaked} leaked: {summary}");
    }
    let src = mcphost::doctor::error_summary("tool_exception", "E: boom", &data);
    assert!(!src.contains("SUPERSECRET") && src.contains("secret=***"), "{src}");
}
