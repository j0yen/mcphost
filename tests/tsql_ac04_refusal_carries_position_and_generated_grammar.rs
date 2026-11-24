//! contract: reads registry handler::host_tool_descriptors
//! PRD-mcphost-table-sql-exec-and-grammar
//! AC4 -- Given `SELEC * FROM t`, When `host.table.query` runs, Then the refusal
//! carries `data.line = 1`, `data.column = 1` and `data.allowed` equal to
//! `allowed_statement_forms()`; a test parses each `allowed` form back to a
//! statement kind and asserts the set equals the validator's accepted set.

use std::collections::BTreeSet;

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::handler::host_tool_descriptors;
use mcphost::kinds::KindRegistry;
use mcphost::table::exec::{
    EXEC_ALLOWED, FORMS, QUERY_ALLOWED, StatementKind, accepted_kind, allowed_kinds_from_forms, allowed_statement_forms,
    parser_location,
};
use serde_json::{Value, json};

fn strings(value: &Value) -> Vec<String> {
    value.as_array().expect("an array").iter().map(|v| v.as_str().expect("a string").to_string()).collect()
}

#[tokio::test]
async fn a_misspelled_select_is_refused_with_its_position_and_the_generated_grammar() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Tsql AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call("host.table.create", json!({"name": "t", "columns": {"id": "integer"}}))
        .await
        .expect("create t");

    let err = client
        .tools_call("host.table.query", json!({"sql": "SELEC * FROM t"}))
        .await
        .expect_err("a misspelled SELECT is refused");
    assert_eq!(err.error_code.as_deref(), Some("table_query_rejected"), "{err:?}");
    assert_eq!(err.data["line"], 1, "{err:?}");
    assert_eq!(err.data["column"], 1, "{err:?}");
    assert!(err.data["reason"].as_str().is_some_and(|r| r.contains("SELEC")), "the reason keeps the parser's words: {err:?}");
    assert_eq!(strings(&err.data["allowed"]), allowed_statement_forms(), "{err:?}");

    // The same refusal from exec carries the same position and grammar.
    let exec = client
        .tools_call("host.table.exec", json!({"sql": "SELEC * FROM t"}))
        .await
        .expect_err("exec refuses it too");
    assert_eq!(exec.error_code.as_deref(), Some("table_exec_rejected"), "{exec:?}");
    assert_eq!((exec.data["line"].as_i64(), exec.data["column"].as_i64()), (Some(1), Some(1)), "{exec:?}");
    assert_eq!(strings(&exec.data["allowed"]), allowed_statement_forms(), "{exec:?}");

    // A refusal the parser has no position for says null, not zero.
    let no_position = client
        .tools_call("host.table.query", json!({"sql": "DELETE FROM t WHERE id = 1"}))
        .await
        .expect_err("a write through query is refused");
    assert_eq!(no_position.error_code.as_deref(), Some("table_query_rejected"), "{no_position:?}");
    assert!(no_position.data["line"].is_null() && no_position.data["column"].is_null(), "{no_position:?}");
    assert_eq!(strings(&no_position.data["allowed"]), allowed_statement_forms());
}

#[test]
fn parser_location_is_pinned_to_a_fixed_bad_statement() {
    let error = sqlparser::parser::Parser::parse_sql(&sqlparser::dialect::GenericDialect {}, "SELEC * FROM t")
        .expect_err("a misspelled SELECT does not parse");
    assert_eq!(parser_location(&error), Some((1, 1)), "an upstream change to the Line/Column text fails here: {error}");
}

/// Statements neither verb may accept, one per shape the AST gate has to name.
const REFUSED: &[&str] = &[
    "CREATE TABLE u (x integer)",
    "DROP TABLE t",
    "ALTER TABLE t ADD COLUMN b integer",
    "PRAGMA query_only = OFF",
    "ATTACH DATABASE '/tmp/x.db' AS x",
    "CREATE TRIGGER g AFTER INSERT ON t BEGIN DELETE FROM t; END",
    "UPDATE t SET a = 1",
    "DELETE FROM t",
    "REPLACE INTO t (id, a) VALUES (1, 1)",
    "SELEC * FROM t",
    "",
];

#[test]
fn the_kinds_the_allowed_forms_spell_equal_the_kinds_the_validators_accept() {
    // 1. Parse each `allowed` form back to a statement kind.
    let from_forms: BTreeSet<StatementKind> = allowed_statement_forms()
        .iter()
        .map(|form| StatementKind::from_leading_keyword(form).unwrap_or_else(|| panic!("form {form:?} opens with no statement keyword")))
        .collect();
    assert_eq!(from_forms, allowed_kinds_from_forms().into_iter().collect::<BTreeSet<_>>(), "each form reads back as its row's kind");

    // 2. Probe the validators with every form's example and every refused shape.
    let probes: Vec<&str> = FORMS.iter().map(|f| f.example).chain(REFUSED.iter().copied()).collect();
    let accepted: BTreeSet<StatementKind> = probes.iter().filter_map(|sql| accepted_kind(sql)).collect();
    assert_eq!(accepted, from_forms, "the validators accept exactly the kinds the grammar lists");

    // 3. Every form's example is accepted as that form's kind; every refused shape is refused.
    for f in FORMS {
        assert_eq!(accepted_kind(f.example), Some(f.kind), "the example for {:?} must validate as {:?}", f.form, f.kind);
        assert_eq!(StatementKind::from_leading_keyword(f.form), Some(f.kind), "{:?}", f.form);
    }
    for sql in REFUSED {
        assert_eq!(accepted_kind(sql), None, "{sql:?} must be refused by both verbs");
    }

    // 4. The kind lists the validators read are the kinds the grammar spells.
    let listed: BTreeSet<StatementKind> = EXEC_ALLOWED.iter().chain(QUERY_ALLOWED).copied().collect();
    assert_eq!(listed, from_forms, "EXEC_ALLOWED + QUERY_ALLOWED are the grammar's kinds");
}

#[test]
fn the_tool_descriptions_carry_the_generated_grammar() {
    let tools = host_tool_descriptors(&KindRegistry::with_builtin());
    let description = |name: &str| {
        tools
            .iter()
            .find(|t| t.name.as_ref() == name)
            .and_then(|t| t.description.as_ref())
            .map(|d| d.to_string())
            .unwrap_or_else(|| panic!("{name} has no description"))
    };
    let exec = description("host.table.exec");
    for form in allowed_statement_forms() {
        assert!(exec.contains(&form), "host.table.exec's description lists {form:?}: {exec}");
    }
    let query = description("host.table.query");
    assert!(query.contains("data.allowed") && query.contains("data.line"), "host.table.query's description names the refusal fields: {query}");
}
