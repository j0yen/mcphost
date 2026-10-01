//! PRD-mcphost-lineage-blast-radius requirement 4 / technical
//! considerations ("source scanning"): a scan over a `python` tool's
//! source for `mcphost.table.*` calls, collecting the table names their
//! SQL string literals reference. Dynamic table names (an f-string, a
//! variable) are exactly why `reads` exists as a declared fallback -- this
//! scan only ever sees a literal string, and reuses `sqlparser` (already a
//! dependency, `tables.rs`'s own `host.table.query` structural check) to
//! read table names out of it rather than hand-rolling a second SQL
//! grammar alongside a regex.

const CALLS: &[&str] = &["mcphost.table.query(", "mcphost.table.chart(", "mcphost.table.handle("];

/// Finds every `mcphost.table.{query,chart,handle}(` call site in `source`
/// whose first argument is a plain (optionally `f`-prefixed) string
/// literal, and returns that literal's contents.
fn string_literal_args(source: &str) -> Vec<String> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    for call in CALLS {
        let mut search_from = 0usize;
        while let Some(rel) = source[search_from..].find(call) {
            let call_end = search_from + rel + call.len();
            let mut i = call_end;
            while i < bytes.len() && (bytes[i] as char).is_whitespace() {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'f' {
                i += 1;
            }
            if i < bytes.len() && (bytes[i] == b'"' || bytes[i] == b'\'') {
                let quote = bytes[i];
                let start = i + 1;
                let mut j = start;
                while j < bytes.len() && bytes[j] != quote {
                    if bytes[j] == b'\\' {
                        j += 1;
                    }
                    j += 1;
                }
                if j <= bytes.len() {
                    out.push(source[start..j.min(bytes.len())].to_string());
                }
            }
            search_from = call_end;
        }
    }
    out
}

/// Recursively collects every table name referenced by a `FROM`/`JOIN` in
/// a parsed `sqlparser` query, including CTEs and set operations
/// (`UNION`/etc). Best-effort: an unrecognized construct is simply not
/// walked into, never an error.
fn collect_from_query(query: &sqlparser::ast::Query, out: &mut Vec<String>) {
    if let Some(with) = &query.with {
        for cte in &with.cte_tables {
            collect_from_query(&cte.query, out);
        }
    }
    collect_from_set_expr(&query.body, out);
}

fn collect_from_set_expr(expr: &sqlparser::ast::SetExpr, out: &mut Vec<String>) {
    use sqlparser::ast::SetExpr;
    match expr {
        SetExpr::Select(select) => {
            for twj in &select.from {
                collect_from_table_factor(&twj.relation, out);
                for join in &twj.joins {
                    collect_from_table_factor(&join.relation, out);
                }
            }
        }
        SetExpr::Query(q) => collect_from_query(q, out),
        SetExpr::SetOperation { left, right, .. } => {
            collect_from_set_expr(left, out);
            collect_from_set_expr(right, out);
        }
        _ => {}
    }
}

fn collect_from_table_factor(factor: &sqlparser::ast::TableFactor, out: &mut Vec<String>) {
    use sqlparser::ast::TableFactor;
    match factor {
        TableFactor::Table { name, .. } => {
            if let Some(last) = name.0.last() {
                out.push(last.to_string().trim_matches(|c| c == '"' || c == '`' || c == '[' || c == ']').to_string());
            }
        }
        TableFactor::Derived { subquery, .. } => collect_from_query(subquery, out),
        _ => {}
    }
}

/// Returns every distinct table name referenced by a `mcphost.table.*`
/// call's literal SQL argument in `source`, in first-seen order.
/// Unparseable literals (not valid SQL, e.g. a non-SQL string passed to
/// `.handle(...)`) are silently skipped -- best-effort, never fatal to a
/// publish.
pub fn scan_python_source_for_tables(source: &str) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for literal in string_literal_args(source) {
        let Ok(statements) = sqlparser::parser::Parser::parse_sql(&sqlparser::dialect::GenericDialect {}, &literal)
        else {
            continue;
        };
        for stmt in &statements {
            if let sqlparser::ast::Statement::Query(q) = stmt {
                let mut names = Vec::new();
                collect_from_query(q, &mut names);
                for name in names {
                    if seen.insert(name.clone()) {
                        out.push(name);
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_table_from_a_query_call() {
        let source = r#"mcphost.table.query("SELECT * FROM orders")"#;
        assert_eq!(scan_python_source_for_tables(source), vec!["orders".to_string()]);
    }

    #[test]
    fn finds_multiple_distinct_tables_across_calls() {
        let source = r#"
            mcphost.table.query("SELECT * FROM orders JOIN customers ON 1=1")
            mcphost.table.query("SELECT * FROM orders")
        "#;
        assert_eq!(
            scan_python_source_for_tables(source),
            vec!["orders".to_string(), "customers".to_string()]
        );
    }

    #[test]
    fn ignores_dynamic_table_names() {
        let source = "mcphost.table.query(build_sql())";
        assert!(scan_python_source_for_tables(source).is_empty());
    }
}
