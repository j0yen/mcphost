//! PRD-mcphost-example-from-published-tool
//! AC2 — Given a tool whose source contains the literal value of a stored
//! secret, When `host.example.propose` runs, Then it returns
//! `secret_leak {line}` with no value echoed and no proposal row.

use crate::exprp;

use serde_json::json;

#[tokio::test]
async fn a_hard_coded_secret_value_is_refused_with_its_line_and_no_row() {
    let f = exprp::start().await;
    let mut spec = exprp::probe_spec(&f.upstream);
    spec["headers"] = json!({"Authorization": format!("Bearer {}", exprp::TOKEN_VALUE)});
    exprp::publish_http(&f.client, "hardcoded", spec).await;

    let err = exprp::propose(&f.client, exprp::propose_args("hardcoded"))
        .await
        .expect_err("a literal secret must be refused");
    assert_eq!(err.error_code.as_deref(), Some("secret_leak"), "{} {}", err.code, err.message);
    let line = err.data["line"].as_u64().expect("data.line");
    assert!(line >= 1, "line is 1-based: {}", err.data);
    assert_eq!(err.data["secret"], "TOKEN", "names the secret, not its value");
    let everything = format!("{} {}", err.message, err.data);
    assert!(!everything.contains(exprp::TOKEN_VALUE), "the value must not be echoed: {everything}");

    let rows = f.server.state.db.list_example_proposals(None).await.expect("list");
    assert!(rows.is_empty(), "a refused proposal stores no row: {rows:?}");
}

#[test]
fn the_scan_finds_a_value_on_the_right_line_and_ignores_reference_text() {
    let secrets = vec![("TOKEN".to_string(), exprp::TOKEN_VALUE.to_string())];
    let text = format!("a = 1\nb = \"{}\"\n", exprp::TOKEN_VALUE);
    let hit = mcphost::example_propose::find_secret_leak(&[("tools/x.py", text.as_str())], &secrets);
    assert_eq!(hit, Some(("tools/x.py".to_string(), 2, "TOKEN".to_string())));
    let clean = mcphost::example_propose::find_secret_leak(&[("tools/x.py", "t = '{{ secret.TOKEN }}'")], &secrets);
    assert_eq!(clean, None);
}
