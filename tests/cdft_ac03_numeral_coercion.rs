//! PRD-mcphost-chain-friction-fixes-20261016 AC3 (P0) -- canonical numeral
//! strings are coerced to the column type by one shared function, reported
//! in `coerced`, and everything else keeps today's schema violation, through
//! `host.table.append` and `host.state.insert` alike.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

async fn setup() -> (TestServer, McpClient) {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Cdft AC3").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.table.create",
            json!({"name": "costs", "columns": {"cost_usd": "real", "n": "integer", "ok": "boolean"}}),
        )
        .await
        .expect("table create");
    client
        .tools_call(
            "host.state.table_create",
            json!({"name": "costs", "schema": {"cost_usd": "real", "n": "integer", "ok": "boolean"}}),
        )
        .await
        .expect("state table create");
    (server, client)
}

fn coerced_fields(body: &Value) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = body["coerced"]
        .as_array()
        .expect("coerced")
        .iter()
        .map(|c| {
            assert_eq!(c["row"], json!(0));
            assert_eq!(c["from"], json!("string"));
            (c["field"].as_str().unwrap().to_string(), c["to"].as_str().unwrap().to_string())
        })
        .collect();
    v.sort();
    v
}

fn want_coerced() -> Vec<(String, String)> {
    vec![
        ("cost_usd".into(), "real".into()),
        ("n".into(), "integer".into()),
        ("ok".into(), "boolean".into()),
    ]
}

#[tokio::test]
async fn table_append_coerces_and_reports() {
    let (_s, client) = setup().await;
    let row = json!({"cost_usd": "0.0012", "n": "42", "ok": "true"});
    let body = extract_structured(
        &client.tools_call("host.table.append", json!({"table": "costs", "rows": [row]})).await.expect("append"),
    );
    assert_eq!(coerced_fields(&body), want_coerced());
    let q = extract_structured(
        &client
            .tools_call("host.table.query", json!({"sql": "SELECT cost_usd, n, ok FROM costs"}))
            .await
            .expect("query"),
    );
    let stored = &q["rows"][0];
    let cells: Vec<Value> = match stored {
        Value::Array(a) => a.clone(),
        Value::Object(o) => vec![o["cost_usd"].clone(), o["n"].clone(), o["ok"].clone()],
        other => panic!("row shape {other}"),
    };
    assert_eq!(cells, vec![json!(0.0012), json!(42), json!(1)], "{q}");
}

#[tokio::test]
async fn state_insert_coerces_and_reports_identically() {
    let (_s, client) = setup().await;
    let row = json!({"cost_usd": "0.0012", "n": "42", "ok": "true"});
    let body = extract_structured(
        &client.tools_call("host.state.insert", json!({"table": "costs", "rows": [row]})).await.expect("insert"),
    );
    assert_eq!(coerced_fields(&body), want_coerced());
    let q = extract_structured(
        &client.tools_call("host.state.query", json!({"table": "costs"})).await.expect("query"),
    );
    let stored = &q["rows"][0];
    let stored = stored.get("row").unwrap_or(stored);
    assert_eq!(stored["cost_usd"], json!(0.0012), "{q}");
    assert_eq!(stored["n"], json!(42), "{q}");
    assert_eq!(stored["ok"], json!(true), "{q}");
}

#[tokio::test]
async fn non_canonical_strings_fail_identically_through_both_paths() {
    let (_s, client) = setup().await;
    for (row, field, ty) in [
        (json!({"cost_usd": "abc"}), "cost_usd", "real"),
        (json!({"n": "4.2"}), "n", "integer"),
        (json!({"n": " 42 "}), "n", "integer"),
        (json!({"n": "1,000"}), "n", "integer"),
        (json!({"ok": "yes"}), "ok", "boolean"),
    ] {
        let t = client
            .tools_call("host.table.append", json!({"table": "costs", "rows": [row.clone()]}))
            .await
            .expect_err("table append refuses");
        let s = client
            .tools_call("host.state.insert", json!({"table": "costs", "rows": [row]}))
            .await
            .expect_err("state insert refuses");
        assert_eq!(t.error_code.as_deref(), Some("table_schema_violation"), "{t:?}");
        assert_eq!(s.error_code.as_deref(), Some("state_schema_violation"), "{s:?}");
        let want = format!("column '{field}' must be of type '{ty}'");
        assert!(t.message.contains(&want) && t.message.ends_with("got string"), "{}", t.message);
        assert_eq!(t.message, s.message, "one shared message");
    }
}

/// Drift test: the two modules' coercion is one function, so for every
/// declared type both agree on every probe.
#[test]
fn shared_function_semantics() {
    let cases: [(&str, &str, Option<Value>); 14] = [
        ("integer", "42", Some(json!(42))),
        ("integer", "-7", Some(json!(-7))),
        ("integer", "+7", Some(json!(7))),
        ("integer", "4.2", None),
        ("integer", "1e3", None),
        ("integer", " 42", None),
        ("integer", "", None),
        ("real", "0.0012", Some(json!(0.0012))),
        ("real", "3", Some(json!(3))),
        ("real", "1,000", None),
        ("real", " 1.5", None),
        ("boolean", "false", Some(json!(false))),
        ("boolean", "True", None),
        ("text", "42", None),
    ];
    for (ty, text, want) in cases {
        assert_eq!(mcphost::tables::coerce_canonical_numeral(ty, &json!(text)), want, "{ty} {text:?}");
    }
    assert_eq!(mcphost::tables::coerce_canonical_numeral("integer", &json!(4)), None);
}
