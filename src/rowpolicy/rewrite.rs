//! requirement 4 (AC4): rewrites a parsed `Query`'s AST so every
//! `TableFactor::Table` naming a policied table becomes a derived table
//! `(SELECT * FROM t WHERE <predicate>) AS t` -- never string concatenation
//! of the predicate into the caller's SQL text. Rides sqlparser's own
//! `Visit`/`VisitMut` (the `visitor` feature, `Cargo.toml`), which recurses
//! into CTEs, set operations and expression-level subqueries for every AST
//! node that derives it, rather than a second hand-rolled walker alongside
//! the crate's own.

use std::collections::HashMap;
use std::ops::ControlFlow;

use serde_json::Value;
use sqlparser::ast::{Ident, Query, Statement, TableAlias, TableFactor, Visit, VisitMut, VisitorMut};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;

use crate::errors::AppError;

use super::policy::CompiledPolicy;

fn object_name_last(name: &sqlparser::ast::ObjectName) -> Option<String> {
    name.0.last().and_then(|p| p.as_ident()).map(|i| i.value.clone())
}

/// requirement 4: every distinct table name this query's AST references --
/// a read-only pass so a policy lookup (async, needs the DB) never has to
/// run inside the synchronous mutation pass below.
pub fn referenced_table_names(query: &Query) -> Vec<String> {
    struct Collector {
        names: Vec<String>,
    }
    impl sqlparser::ast::Visitor for Collector {
        type Break = ();
        fn pre_visit_table_factor(
            &mut self,
            table_factor: &TableFactor,
        ) -> ControlFlow<Self::Break> {
            if let TableFactor::Table { name, .. } = table_factor
                && let Some(last) = object_name_last(name)
            {
                let lower = last.to_lowercase();
                if !self.names.contains(&lower) {
                    self.names.push(lower);
                }
            }
            ControlFlow::Continue(())
        }
    }
    let mut collector = Collector { names: Vec::new() };
    let _ = Visit::visit(query, &mut collector);
    collector.names
}

pub struct Rewritten {
    pub sql: String,
    /// `(":name", value)` pairs to bind on the rewritten SQL -- literals
    /// never appear directly in `sql` itself (requirement 4).
    pub bindings: Vec<(String, Value)>,
    pub rewrote_any: bool,
}

struct PolicyRewriter<'a> {
    compiled: &'a HashMap<String, CompiledPolicy>,
    bindings: Vec<(String, Value)>,
    occurrence: usize,
    rewrote_any: bool,
    error: Option<AppError>,
}

fn parse_error(reason: impl Into<String>) -> AppError {
    let reason = reason.into();
    AppError::Internal(format!("row policy rewrite failed to build a derived subquery: {reason}"))
}

impl VisitorMut for PolicyRewriter<'_> {
    type Break = ();

    /// Rewriting in `post_visit` (not `pre_visit`) matters: sqlparser's
    /// derived `VisitMut` for `TableFactor` runs pre-visit, then recurses
    /// into whatever variant `self` holds *at that point* (so a `pre_visit`
    /// rewrite from `Table` to `Derived` would be immediately re-descended
    /// into by the very same `visit()` call, finding the fresh `orders`
    /// reference inside the derived subquery it just built and rewriting
    /// that too -- forever). Post-visit runs after that one recursion step
    /// has already completed against the original `Table` node (which has
    /// no subquery to recurse into), so swapping it to `Derived` here is
    /// never seen by this pass again.
    fn post_visit_table_factor(&mut self, table_factor: &mut TableFactor) -> ControlFlow<Self::Break> {
        if self.error.is_some() {
            return ControlFlow::Break(());
        }
        let TableFactor::Table { name, alias, .. } = table_factor else {
            return ControlFlow::Continue(());
        };
        let Some(table_name) = object_name_last(name) else {
            return ControlFlow::Continue(());
        };
        let lower = table_name.to_lowercase();
        let Some(compiled) = self.compiled.get(&lower) else {
            return ControlFlow::Continue(());
        };

        let occ = self.occurrence;
        self.occurrence += 1;

        let mut predicate_text = compiled.rls_predicate.clone();
        for (i, value) in compiled.rls_params.iter().enumerate() {
            let placeholder = format!("{{p{i}}}");
            let bound_name = format!(":rp{occ}_{i}");
            predicate_text = predicate_text.replace(&placeholder, &bound_name);
            self.bindings.push((bound_name, value.clone()));
        }

        let quoted_table = table_name.replace('"', "\"\"");
        let subquery_sql = format!("SELECT * FROM \"{quoted_table}\" WHERE {predicate_text}");
        let inner_query = match Parser::parse_sql(&GenericDialect {}, &subquery_sql) {
            Ok(mut statements) if statements.len() == 1 => match statements.remove(0) {
                Statement::Query(q) => q,
                _ => {
                    self.error = Some(parse_error("predicate did not parse back as a query"));
                    return ControlFlow::Break(());
                }
            },
            _ => {
                self.error = Some(parse_error("predicate failed to parse"));
                return ControlFlow::Break(());
            }
        };

        let table_alias = alias.clone().unwrap_or(TableAlias {
            explicit: true,
            name: Ident::new(table_name.clone()),
            columns: Vec::new(),
            at: None,
        });

        *table_factor = TableFactor::Derived {
            lateral: false,
            subquery: inner_query,
            alias: Some(table_alias),
            sample: None,
        };
        self.rewrote_any = true;
        ControlFlow::Continue(())
    }
}

/// requirement 4: applies `compiled` (one entry per policied table this
/// query references) to `query` in place. Returns the rewritten SQL text
/// plus the bindings to execute it with -- `rewrote_any` is `false` (and
/// `sql` textually unchanged) when `compiled` is empty, e.g. a query that
/// references no policied table.
pub fn apply(query: &mut Query, compiled: &HashMap<String, CompiledPolicy>) -> Result<Rewritten, AppError> {
    let mut rewriter =
        PolicyRewriter { compiled, bindings: Vec::new(), occurrence: 0, rewrote_any: false, error: None };
    let _ = VisitMut::visit(query, &mut rewriter);
    if let Some(err) = rewriter.error {
        return Err(err);
    }
    Ok(Rewritten { sql: query.to_string(), bindings: rewriter.bindings, rewrote_any: rewriter.rewrote_any })
}
