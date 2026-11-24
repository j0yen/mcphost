//! `host.table.find`: query by example.
//!
//! PRD-mcphost-table-sql-exec-and-grammar R4. `where` is an object of column to
//! value (equality) or column to `{operator: value}`; it compiles to one
//! parameterised SELECT whose values are bound by name, never spliced into the
//! text, and runs through the same path `host.table.query` uses (row policies,
//! the query log, the page total). The response carries the SQL it compiled under
//! `data.sql` so an agent can learn the SQL form.

use serde_json::{Map, Value, json};

use crate::db::Tenant;
use crate::enduser::EndUser;
use crate::errors::AppError;
use crate::state::AppState;
use crate::table::exec::{Verb, rejected_with};
use crate::tables;

/// R4: the comparison operators a `where` entry may use.
pub const ALLOWED_OPERATORS: &[&str] = &["=", "!=", "<", "<=", ">", ">=", "like", "in"];

/// Rows returned when `limit` is omitted.
pub const FIND_LIMIT_DEFAULT: i64 = 100;

/// A compiled `find`: the SELECT (without `LIMIT`), its named bindings, the row limit.
#[derive(Debug, PartialEq)]
pub struct Compiled {
    pub sql: String,
    pub bindings: Vec<(String, Value)>,
    pub limit: i64,
}

impl Compiled {
    /// The statement a caller could send to `host.table.query` (values still bound by name).
    pub fn display_sql(&self) -> String {
        format!("{} LIMIT {}", self.sql, self.limit)
    }
}

fn refused(reason: String, columns: &[String], extra: Value) -> AppError {
    let mut data = json!({"allowed_operators": ALLOWED_OPERATORS, "columns": columns});
    if let (Some(data), Value::Object(extra)) = (data.as_object_mut(), extra) {
        data.extend(extra);
    }
    rejected_with(Verb::Query, reason, data)
}

fn is_scalar(value: &Value) -> bool {
    matches!(value, Value::String(_) | Value::Number(_) | Value::Bool(_))
}

/// Compiles `args` (`where`, `limit`, `order`) over `table` whose declared columns are `columns`.
pub fn compile(table: &str, columns: &[String], args: &Value) -> Result<Compiled, AppError> {
    let mut bindings: Vec<(String, Value)> = Vec::new();
    let mut clauses: Vec<String> = Vec::new();
    let empty = Map::new();
    let entries = match args.get("where") {
        None | Some(Value::Null) => &empty,
        Some(Value::Object(map)) => map,
        Some(_) => {
            return Err(refused(
                "where must be an object: {\"column\": value} or {\"column\": {\"<operator>\": value}}".to_string(),
                columns,
                json!({}),
            ));
        }
    };
    for (column, spec) in entries {
        if !columns.contains(column) {
            return Err(refused(
                format!("table '{table}' has no column '{column}'"),
                columns,
                json!({"table": table, "column": column}),
            ));
        }
        match spec {
            Value::Object(operators) if operators.is_empty() => {
                return Err(refused(
                    format!("column '{column}' names no operator; use one of {}", ALLOWED_OPERATORS.join(", ")),
                    columns,
                    json!({"table": table, "column": column}),
                ));
            }
            Value::Object(operators) => {
                for (operator, value) in operators {
                    clauses.push(clause(column, operator, value, &mut bindings, columns)?);
                }
            }
            Value::Array(_) => clauses.push(clause(column, "in", spec, &mut bindings, columns)?),
            other => clauses.push(clause(column, "=", other, &mut bindings, columns)?),
        }
    }

    let mut sql = format!("SELECT * FROM {}", tables::quote_ident(table));
    if !clauses.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&clauses.join(" AND "));
    }
    let order = order_by(args.get("order"), columns)?;
    if !order.is_empty() {
        sql.push_str(" ORDER BY ");
        sql.push_str(&order.join(", "));
    }
    let limit = match args.get("limit") {
        None | Some(Value::Null) => FIND_LIMIT_DEFAULT,
        Some(value) => match value.as_i64() {
            Some(n) if (1..=tables::ROW_CAP).contains(&n) => n,
            _ => return Err(AppError::InvalidArgs(format!("limit: must be an integer from 1 to {}", tables::ROW_CAP))),
        },
    };
    Ok(Compiled { sql, bindings, limit })
}

/// One `where` entry: `"column" <operator> :exN`.
fn clause(
    column: &str,
    operator: &str,
    value: &Value,
    bindings: &mut Vec<(String, Value)>,
    columns: &[String],
) -> Result<String, AppError> {
    let quoted = tables::quote_ident(column);
    let op = operator.trim().to_ascii_lowercase();
    let bad_value = |why: &str| {
        refused(
            format!("column '{column}': operator '{op}' {why}"),
            columns,
            json!({"column": column, "operator": op}),
        )
    };
    let mut bind = |value: &Value| {
        let name = format!(":ex{}", bindings.len());
        bindings.push((name.clone(), value.clone()));
        name
    };
    match op.as_str() {
        "=" if value.is_null() => Ok(format!("{quoted} IS NULL")),
        "!=" if value.is_null() => Ok(format!("{quoted} IS NOT NULL")),
        "=" | "!=" | "<" | "<=" | ">" | ">=" => {
            if !is_scalar(value) {
                return Err(bad_value("takes one string, number or boolean"));
            }
            Ok(format!("{quoted} {op} {}", bind(value)))
        }
        "like" => {
            if !value.is_string() {
                return Err(bad_value("takes one string pattern"));
            }
            Ok(format!("{quoted} LIKE {}", bind(value)))
        }
        "in" => {
            let Some(items) = value.as_array().filter(|items| !items.is_empty() && items.iter().all(is_scalar)) else {
                return Err(bad_value("takes a non-empty array of strings, numbers or booleans"));
            };
            let names: Vec<String> = items.iter().map(&mut bind).collect();
            Ok(format!("{quoted} IN ({})", names.join(", ")))
        }
        _ => Err(refused(
            format!("unknown operator '{operator}' for column '{column}'; use one of {}", ALLOWED_OPERATORS.join(", ")),
            columns,
            json!({"column": column, "operator": operator}),
        )),
    }
}

/// `order`: a column name, `"column desc"`, or an array of those.
fn order_by(order: Option<&Value>, columns: &[String]) -> Result<Vec<String>, AppError> {
    let specs: Vec<&Value> = match order {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(items)) => items.iter().collect(),
        Some(one) => vec![one],
    };
    let mut out = Vec::new();
    for spec in specs {
        let text = spec.as_str().unwrap_or_default();
        let mut parts = text.split_whitespace();
        let (column, direction) = (parts.next().unwrap_or_default(), parts.next().unwrap_or("asc").to_ascii_lowercase());
        if parts.next().is_some() || !matches!(direction.as_str(), "asc" | "desc") || !columns.iter().any(|c| c == column) {
            return Err(refused(
                format!("order: '{text}' is not a column of this table, optionally followed by asc or desc"),
                columns,
                json!({"order": spec}),
            ));
        }
        out.push(format!("{} {}", tables::quote_ident(column), direction.to_ascii_uppercase()));
    }
    Ok(out)
}

/// `host.table.find {table, where?, limit?, order?}`.
pub async fn table_find(
    state: &AppState,
    tenant: &Tenant,
    args: &Value,
    end_user: Option<&EndUser>,
) -> Result<Value, AppError> {
    let table = tables::arg_str(args, "table")?;
    let path = tables::tenant_db_path(state, tenant.id);
    let table_for_conn = table.clone();
    let columns: Vec<String> = tables::with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
        Ok(tables::load_schema_sync(conn, &table_for_conn)?.columns.into_keys().collect())
    })
    .await?;

    let compiled = compile(&table, &columns, args)?;
    let mut out = tables::table_query_bound(state, tenant, &compiled, end_user).await?;
    let params: Map<String, Value> = compiled.bindings.iter().cloned().collect();
    if let Some(obj) = out.as_object_mut() {
        obj.insert("data".to_string(), json!({"sql": compiled.display_sql(), "params": params}));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cols() -> Vec<String> {
        ["amount", "id", "status"].map(String::from).to_vec()
    }

    #[test]
    fn compiles_the_by_example_object_to_a_parameterised_select() {
        let c = compile("t", &cols(), &json!({"where": {"amount": {">": 500}, "status": "open"}})).unwrap();
        assert_eq!(c.sql, "SELECT * FROM \"t\" WHERE \"amount\" > :ex0 AND \"status\" = :ex1");
        assert_eq!(c.bindings, vec![(":ex0".to_string(), json!(500)), (":ex1".to_string(), json!("open"))]);
        assert_eq!(c.limit, FIND_LIMIT_DEFAULT);
    }

    #[test]
    fn values_are_never_spliced_into_the_text() {
        let c = compile("t", &cols(), &json!({"where": {"status": "x' OR '1'='1"}})).unwrap();
        assert!(!c.sql.contains("OR '1'"), "{}", c.sql);
        assert_eq!(c.bindings[0].1, json!("x' OR '1'='1"));
    }

    #[test]
    fn in_like_null_and_order_compile() {
        let c = compile(
            "t",
            &cols(),
            &json!({"where": {"id": [1, 2], "status": {"like": "o%", "!=": null}}, "order": ["amount desc", "id"], "limit": 5}),
        )
        .unwrap();
        assert_eq!(
            c.sql,
            "SELECT * FROM \"t\" WHERE \"id\" IN (:ex0, :ex1) AND \"status\" IS NOT NULL AND \"status\" LIKE :ex2 \
             ORDER BY \"amount\" DESC, \"id\" ASC"
        );
        assert_eq!(c.limit, 5);
    }

    #[test]
    fn the_row_policy_rewrite_keeps_findings_named_placeholders() {
        use crate::rowpolicy::{PolicyRule, PolicyTarget, RowPolicy, RuleOp, RuleValue, SecurityContext, policy, rewrite};
        let compiled = compile("orders", &cols(), &json!({"where": {"amount": {">": 500}}})).unwrap();
        let mut statements =
            sqlparser::parser::Parser::parse_sql(&sqlparser::dialect::GenericDialect {}, &compiled.sql).unwrap();
        let sqlparser::ast::Statement::Query(mut query) = statements.remove(0) else { panic!("a query") };
        let row_policy = RowPolicy {
            id: 1,
            tenant_id: 1,
            version: 1,
            target: PolicyTarget::Table("orders".to_string()),
            rule: vec![PolicyRule {
                column_or_attr: "status".to_string(),
                op: RuleOp::Eq,
                value: RuleValue::Attr("status".to_string()),
            }],
            created_unix: 0,
        };
        let ctx = SecurityContext {
            subject: "alice".to_string(),
            issuer: None,
            method: "assertion".to_string(),
            attrs: [("status".to_string(), json!("open"))].into_iter().collect(),
        };
        let mut map = std::collections::HashMap::new();
        map.insert("orders".to_string(), policy::compile(&row_policy, &ctx));
        let outcome = rewrite::apply(&mut query, &map).unwrap();
        assert!(outcome.sql.contains(":ex0"), "the find placeholder survives the rewrite: {}", outcome.sql);
        assert!(outcome.bindings.iter().all(|(name, _)| name != ":ex0"), "policy bindings never reuse :exN");
    }

    #[test]
    fn an_unknown_operator_lists_the_operators_and_the_columns() {
        let err = compile("t", &cols(), &json!({"where": {"amount": {"~": 5}}})).unwrap_err();
        let AppError::Structured { code, data, .. } = err else { panic!("structured") };
        assert_eq!(code, "table_query_rejected");
        assert_eq!(data["allowed_operators"], json!(ALLOWED_OPERATORS));
        assert_eq!(data["columns"], json!(["amount", "id", "status"]));
        assert_eq!(data["operator"], "~");
    }
}
