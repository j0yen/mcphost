//! PRD-mcphost-row-policy
//! AC4 (P0) — Given a query with a CTE and a subquery both naming
//! `orders`, When Alice runs it, Then both references are rewritten and
//! the result matches the same query over EU rows only, and the rewrite
//! touches the AST nodes rather than the SQL text.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::Serialize;
use serde_json::json;
use sqlparser::ast::{Expr, SetExpr, Statement, TableFactor};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;
use std::collections::{BTreeMap, HashMap};

const CTE_AND_SUBQUERY_SQL: &str =
    "WITH recent AS (SELECT * FROM orders) SELECT COUNT(*) AS n FROM recent WHERE id IN (SELECT id FROM orders)";

#[derive(Serialize)]
struct AssertionClaims {
    sub: String,
    iat: i64,
    exp: i64,
}

fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

fn sign_assertion(secret: &str, sub: &str) -> String {
    let now = now_unix();
    let claims = AssertionClaims { sub: sub.to_string(), iat: now, exp: now + 300 };
    encode(&Header::new(Algorithm::HS256), &claims, &EncodingKey::from_secret(secret.as_bytes()))
        .expect("sign test assertion")
}

/// Digs `from[0].relation` out of a `Query` whose body is a plain
/// `SELECT` -- used to inspect both the CTE's own body and the WHERE
/// clause's `IN` subquery below.
fn first_relation(query: &sqlparser::ast::Query) -> &TableFactor {
    let SetExpr::Select(select) = query.body.as_ref() else { panic!("expected a plain SELECT body") };
    &select.from.first().expect("a FROM clause").relation
}

#[test]
fn the_ast_rewrite_replaces_both_the_cte_and_subquery_table_nodes_not_the_sql_text() {
    let mut statements = Parser::parse_sql(&GenericDialect {}, CTE_AND_SUBQUERY_SQL).expect("parses");
    let Statement::Query(mut query) = statements.remove(0) else { panic!("expected a query") };

    // Before rewrite: both the CTE body and the WHERE-clause subquery name
    // `orders` as a plain `TableFactor::Table`.
    let cte_query_before = query.with.as_ref().unwrap().cte_tables[0].query.clone();
    assert!(matches!(first_relation(&cte_query_before), TableFactor::Table { .. }));
    let SetExpr::Select(select_before) = query.body.as_ref() else { panic!("expected SELECT") };
    let Some(Expr::InSubquery { subquery: subquery_before, .. }) = &select_before.selection else {
        panic!("expected an IN subquery in the WHERE clause")
    };
    assert!(matches!(first_relation(subquery_before), TableFactor::Table { .. }));

    let policy = mcphost::rowpolicy::RowPolicy {
        id: 1,
        tenant_id: 1,
        version: 1,
        target: mcphost::rowpolicy::PolicyTarget::Table("orders".to_string()),
        rule: vec![mcphost::rowpolicy::PolicyRule {
            column_or_attr: "region".to_string(),
            op: mcphost::rowpolicy::RuleOp::Eq,
            value: mcphost::rowpolicy::RuleValue::Attr("region".to_string()),
        }],
        created_unix: 0,
    };
    let mut attrs = BTreeMap::new();
    attrs.insert("region".to_string(), json!("EU"));
    let ctx = mcphost::rowpolicy::SecurityContext {
        subject: "alice".to_string(),
        issuer: None,
        method: "assertion".to_string(),
        attrs,
    };
    let compiled = mcphost::rowpolicy::policy::compile(&policy, &ctx);
    let mut map = HashMap::new();
    map.insert("orders".to_string(), compiled);

    let outcome = mcphost::rowpolicy::rewrite::apply(&mut query, &map).expect("rewrite ok");
    assert!(outcome.rewrote_any, "the rewrite must report it touched at least one table reference");

    // After rewrite: both occurrences are now `TableFactor::Derived` AST
    // nodes -- the rewrite touched the tree, not the SQL string.
    let cte_query_after = &query.with.as_ref().unwrap().cte_tables[0].query;
    assert!(
        matches!(first_relation(cte_query_after), TableFactor::Derived { .. }),
        "the CTE's own FROM orders must become a derived table in the AST"
    );
    let SetExpr::Select(select_after) = query.body.as_ref() else { panic!("expected SELECT") };
    let Some(Expr::InSubquery { subquery: subquery_after, .. }) = &select_after.selection else {
        panic!("expected an IN subquery in the WHERE clause")
    };
    assert!(
        matches!(first_relation(subquery_after), TableFactor::Derived { .. }),
        "the WHERE-clause subquery's FROM orders must become a derived table in the AST"
    );

    // The predicate's literal value never appears in the emitted SQL text
    // itself -- it travels only as a bound parameter (requirement 4:
    // "string concatenation... is forbidden").
    assert!(
        !outcome.sql.contains("EU"),
        "the rewritten SQL text must never contain the literal attribute value: {}",
        outcome.sql
    );
    assert!(
        outcome.bindings.iter().any(|(_, v)| v == &json!("EU")),
        "the literal value must instead travel as a bound parameter: {:?}",
        outcome.bindings
    );
}

#[tokio::test]
async fn alice_running_the_cte_and_subquery_query_sees_the_same_result_as_the_eu_only_equivalent() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let rotate = client
        .tools_call("host.enduser.assertion_secret_rotate", json!({}))
        .await
        .expect("assertion_secret_rotate ok");
    let secret = extract_structured(&rotate)["secret"].as_str().expect("secret string").to_string();

    client
        .tools_call(
            "host.table.create",
            json!({"name": "orders", "columns": {"id": "integer", "region": "text"}, "primary_key": "id"}),
        )
        .await
        .expect("table create ok");
    client
        .tools_call(
            "host.table.append",
            json!({"table": "orders", "rows": [
                {"id": 1, "region": "EU"},
                {"id": 2, "region": "EU"},
                {"id": 3, "region": "EU"},
                {"id": 4, "region": "US"},
                {"id": 5, "region": "US"},
            ]}),
        )
        .await
        .expect("append ok");
    client
        .tools_call(
            "host.policy.set",
            json!({
                "target": {"table": "orders"},
                "rule": [{"column_or_attr": "region", "op": "eq", "value": {"attr": "region"}}],
            }),
        )
        .await
        .expect("policy set ok");
    client
        .tools_call("host.policy.attrs_set", json!({"subject": "alice", "attrs": {"region": "EU"}}))
        .await
        .expect("attrs set ok");

    let assertion = sign_assertion(&secret, "alice");
    let alice_result = client
        .tools_call(
            "host.table.query",
            json!({"sql": CTE_AND_SUBQUERY_SQL, "end_user_assertion": assertion}),
        )
        .await
        .unwrap_or_else(|e| panic!("alice's query must succeed: {} {}", e.code, e.message));
    let alice_n = extract_structured(&alice_result)["rows"][0]["n"].clone();

    // The same query, hand-written to restrict to EU rows directly (run as
    // the tenant key, so nothing here is itself policy-filtered) -- the
    // baseline "same query over EU rows only" AC4 asks for.
    let baseline_sql = "WITH recent AS (SELECT * FROM orders WHERE region = 'EU') \
         SELECT COUNT(*) AS n FROM recent WHERE id IN (SELECT id FROM orders WHERE region = 'EU')";
    let baseline_result = client
        .tools_call("host.table.query", json!({"sql": baseline_sql}))
        .await
        .expect("baseline query must succeed");
    let baseline_n = extract_structured(&baseline_result)["rows"][0]["n"].clone();

    assert_eq!(alice_n, json!(3), "alice must see exactly the 3 EU rows through both the CTE and the subquery");
    assert_eq!(
        alice_n, baseline_n,
        "alice's rewritten result must match the same query run directly over EU rows only"
    );
}
