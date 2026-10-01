//! PRD-mcphost-query-diagnosis: deterministic, no-model-call diagnosis for
//! a refused or empty `host.table.query` call.
//!
//! requirement 1: for every query the sqlparser gate accepts (whether it
//! then binds cleanly, binds with a "no such table"/"no such column"
//! error, or binds and runs to zero rows), this module collects the table
//! name named in `FROM` and any `column = 'string literal'` equality the
//! `WHERE` clause names, purely from the AST -- no second parse attempt on
//! failure is needed because `host.table.query`'s own gate
//! (`tables::validate_query_structure`) already requires the statement to
//! parse to exactly one `SELECT`/CTE before a bind error can even occur; a
//! genuine SQL *parse* error (malformed syntax) carries no AST to walk and
//! is not diagnosed (no acceptance criterion covers it -- only bind errors
//! and zero-row results do).
//!
//! requirement 2: each identifier is scored against a tenant's own
//! declared names with `strsim::jaro_winkler`, banded `covered` (1.0,
//! exact) / `fuzzy_covered` (>= 0.9) / `partial` (>= 0.75) / `absent`
//! (below) for TABLE and COLUMN identifiers -- the same four-tier scheme
//! `mcp-grounding-eval`'s `CoverageReport` uses for a different surface
//! (measures/dimensions against `describe_model`).
//!
//! requirement 4 (VALUE identifiers) uses a different, BINARY band
//! instead of the four-tier one above -- see [`score_value`]'s doc for
//! why: a SQL equality filter either matches a stored value or it
//! doesn't, so "fuzzy covered" is not a meaningful state for a literal the
//! way it is for a misspelled column name.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use serde_json::{Value, json};
use sqlparser::ast::{BinaryOperator, Expr, SetExpr, Statement, TableFactor, Value as SqlValue};

/// requirement 2: the grounding-eval default thresholds this PRD's own
/// Technical considerations section points at.
pub const SIM_FUZZY_COVERED: f64 = 0.9;
pub const SIM_PARTIAL: f64 = 0.75;
/// requirement 2: "top_candidates (<= 3 {name, similarity})".
pub const MAX_CANDIDATES: usize = 3;
/// requirement 3: "hint ... one sentence, <= 200 characters".
pub const HINT_MAX_CHARS: usize = 200;
/// requirement 4: "bounded to 1,000 distinct" values sampled per column.
pub const DISTINCT_VALUES_BOUND: i64 = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Band {
    Covered,
    FuzzyCovered,
    Partial,
    Absent,
}

impl Band {
    pub fn as_str(self) -> &'static str {
        match self {
            Band::Covered => "covered",
            Band::FuzzyCovered => "fuzzy_covered",
            Band::Partial => "partial",
            Band::Absent => "absent",
        }
    }

    fn from_similarity(sim: f64) -> Self {
        if sim >= 1.0 {
            Band::Covered
        } else if sim >= SIM_FUZZY_COVERED {
            Band::FuzzyCovered
        } else if sim >= SIM_PARTIAL {
            Band::Partial
        } else {
            Band::Absent
        }
    }
}

#[derive(Debug, Clone)]
pub struct Candidate {
    pub name: String,
    pub similarity: f64,
}

#[derive(Debug, Clone)]
pub struct IdentifierDiagnosis {
    pub term: String,
    pub kind: &'static str, // "table" | "column" | "value"
    pub band: Band,
    pub matched_to: Option<String>,
    pub top_candidates: Vec<Candidate>,
}

impl IdentifierDiagnosis {
    pub fn to_json(&self) -> Value {
        json!({
            "term": self.term,
            "kind": self.kind,
            "status": self.band.as_str(),
            "matched_to": self.matched_to,
            "top_candidates": self.top_candidates.iter()
                .map(|c| json!({"name": c.name, "similarity": c.similarity}))
                .collect::<Vec<_>>(),
        })
    }
}

fn round4(x: f64) -> f64 {
    (x * 10_000.0).round() / 10_000.0
}

/// requirement 2/7 (AC7): scores `term` against `(display_name,
/// scoring_text)` candidate pairs -- `scoring_text` differs from
/// `display_name` only for an annotation-derived candidate: a word pulled
/// from a column's `description` annotation scores against `term`, but
/// `matched_to` always reports the owning identifier's real name, never
/// the annotation text itself.
pub fn score_identifier(term: &str, kind: &'static str, candidates: &[(String, String)]) -> IdentifierDiagnosis {
    // Case-insensitive: SQL identifiers (and a human-written annotation
    // sentence like "amount in USD") carry no meaningful case convention
    // of their own -- AC7's `usd` must match the word `USD` inside a
    // description annotation. [`score_value`] below is deliberately the
    // opposite (case-sensitive): a SQL string-literal equality filter IS
    // case-sensitive under SQLite's default collation.
    let term_lower = term.to_lowercase();
    let mut best: BTreeMap<String, f64> = BTreeMap::new();
    for (display, text) in candidates {
        let sim = round4(strsim::jaro_winkler(&term_lower, &text.to_lowercase()));
        best.entry(display.clone())
            .and_modify(|s| {
                if sim > *s {
                    *s = sim;
                }
            })
            .or_insert(sim);
    }
    let mut ranked: Vec<(String, f64)> = best.into_iter().collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal).then_with(|| a.0.cmp(&b.0)));

    let top_candidates: Vec<Candidate> = ranked
        .iter()
        .take(MAX_CANDIDATES)
        .map(|(name, sim)| Candidate { name: name.clone(), similarity: *sim })
        .collect();
    // requirement 2/7 (AC7): the BAND reflects how well `term` matches the
    // real identifier NAME itself, not the boosted score an
    // annotation-word candidate can win ranking with -- `covered`/
    // `fuzzy_covered` should mean the query actually named something
    // close to a real table/column, not merely that identifier's
    // *description* happens to contain a matching word. `top_candidates`
    // above still ranks (and displays) by the boosted score, so the
    // annotation-derived suggestion still surfaces and is still named in
    // the hint -- only the severity band is name-only.
    let (matched_to, band) = match ranked.first() {
        Some((name, _boosted_sim)) => {
            let direct_sim = round4(strsim::jaro_winkler(&term_lower, &name.to_lowercase()));
            (Some(name.clone()), Band::from_similarity(direct_sim))
        }
        None => (None, Band::Absent),
    };

    IdentifierDiagnosis {
        term: term.to_string(),
        kind,
        band,
        matched_to,
        top_candidates,
    }
}

/// requirement 4: diagnoses a WHERE-equality literal against a column's
/// own sampled distinct values.
///
/// Assumption (the PRD's four-tier thresholds, applied literally to AC3's
/// own worked example, disagree with AC3's stated expected band):
/// `jaro_winkler("Produce", "produce")` is 0.9048 -- over the 0.9
/// `fuzzy_covered` threshold requirement 2 gives for TABLE/COLUMN
/// identifiers -- yet AC3 requires this exact pair to band `absent`. A SQL
/// equality filter is boolean (a row either matches or it doesn't), so
/// "closeness" is not a real middle ground for a value the way it is for
/// a misspelled identifier: this function therefore bands `covered` iff
/// an exact (case-sensitive, matching SQLite's default collation) match
/// exists among the sampled distinct values, else `absent` -- never
/// `fuzzy_covered`/`partial`. `top_candidates` is still ranked by
/// Jaro-Winkler regardless of band, so the near-miss (`produce`) still
/// surfaces.
pub fn score_value(term: &str, distinct_values: &[String]) -> IdentifierDiagnosis {
    let exact = distinct_values.iter().any(|v| v == term);
    let candidates: Vec<(String, String)> = distinct_values.iter().map(|v| (v.clone(), v.clone())).collect();
    let mut diag = score_identifier(term, "value", &candidates);
    diag.band = if exact { Band::Covered } else { Band::Absent };
    if exact {
        diag.matched_to = Some(term.to_string());
    }
    diag
}

/// requirement 2/7 (AC7): splits a column's `description` annotation text
/// into word candidates mapped back to that column's real name --
/// `("amount", "amount in USD")` becomes `[("amount","amount"),
/// ("amount","in"), ("amount","USD")]` -- so the identifier `usd` can
/// match the word `USD` inside the sentence via [`score_identifier`]'s
/// case-insensitive comparison, with `matched_to` reporting `amount`.
pub fn annotation_word_candidates(column: &str, description: &str) -> Vec<(String, String)> {
    description
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| (column.to_string(), w.to_string()))
        .collect()
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
}

/// requirement 3: one sentence naming the problem, `None` when there is
/// nothing wrong to report (`Band::Covered`). `table` is the table the
/// identifier was resolved against, when known (AC1's worked example:
/// "column `amout` not found in `expenses`; closest: `amount` (0.96)").
pub fn build_hint(diag: &IdentifierDiagnosis, table: Option<&str>) -> Option<String> {
    if matches!(diag.band, Band::Covered) {
        return None;
    }
    let best = diag.top_candidates.first();
    let in_table = table.map(|t| format!(" in `{t}`")).unwrap_or_default();
    let sentence = match (diag.kind, best) {
        ("table", Some(c)) => {
            format!("table `{}` not found; closest: `{}` ({:.2})", diag.term, c.name, c.similarity)
        }
        ("table", None) => format!("table `{}` not found; no similar table is declared", diag.term),
        ("column", Some(c)) => format!(
            "column `{}` not found{in_table}; closest: `{}` ({:.2})",
            diag.term, c.name, c.similarity
        ),
        ("column", None) => format!("column `{}` not found{in_table}; no similar column is declared", diag.term),
        ("value", Some(c)) => format!(
            "value '{}' not found{in_table}; closest: '{}' ({:.2})",
            diag.term, c.name, c.similarity
        ),
        ("value", None) => format!("value '{}' not found{in_table}; no similar value exists", diag.term),
        _ => format!("`{}` not found", diag.term),
    };
    Some(truncate_chars(&sentence, HINT_MAX_CHARS))
}

/// requirement 1: what [`extract_from_ast`] pulls out of a single
/// `SELECT`/CTE statement -- only the shapes this PRD's ACs need
/// (AC1/AC2/AC7: the queried table; AC3: a `column = 'literal'`
/// top-level-AND equality).
#[derive(Debug, Default, Clone)]
pub struct Extracted {
    pub from_table: Option<String>,
    pub where_equalities: Vec<(String, String)>,
}

/// requirement 1: best-effort AST walk -- `sql` is assumed to already have
/// passed `tables::validate_query_structure` (so it parses to exactly one
/// `SELECT`/CTE); a SQL string that doesn't is simply not diagnosable and
/// returns [`Extracted::default`].
pub fn extract_from_ast(sql: &str) -> Extracted {
    let mut out = Extracted::default();
    let Ok(statements) = sqlparser::parser::Parser::parse_sql(&sqlparser::dialect::GenericDialect {}, sql) else {
        return out;
    };
    let Some(Statement::Query(query)) = statements.into_iter().next() else {
        return out;
    };
    if let SetExpr::Select(select) = *query.body {
        if let Some(twj) = select.from.first()
            && let TableFactor::Table { name, .. } = &twj.relation
        {
            out.from_table = Some(name.to_string());
        }
        if let Some(selection) = &select.selection {
            collect_equalities(selection, &mut out.where_equalities);
        }
    }
    out
}

/// requirement 1/4 (AC3): walks top-level `AND`s collecting every
/// `identifier = 'single-quoted string'` equality -- deliberately not
/// descending into `OR`, parens, or anything else: AC3's own scenario is a
/// single top-level equality, and a richer walk has no acceptance
/// criterion to prove it against.
fn collect_equalities(expr: &Expr, out: &mut Vec<(String, String)>) {
    match expr {
        Expr::BinaryOp {
            left,
            op: BinaryOperator::Eq,
            right,
        } => {
            if let (Expr::Identifier(id), Expr::Value(v)) = (left.as_ref(), right.as_ref())
                && let SqlValue::SingleQuotedString(s) = &v.value
            {
                out.push((id.value.clone(), s.clone()));
            }
        }
        Expr::BinaryOp {
            left,
            op: BinaryOperator::And,
            right,
        } => {
            collect_equalities(left, out);
            collect_equalities(right, out);
        }
        _ => {}
    }
}

/// requirement 1 (bind errors, AC1/AC2/AC7): parses SQLite's own
/// `"no such column: X"` / `"no such table: X"` message text -- the exact
/// strings `rusqlite`/SQLite emit for a syntactically valid statement that
/// names an identifier that doesn't exist (see
/// `tables::run_query_sync`'s `conn.prepare(sql)` failing with this error,
/// surfaced through `AppError::Storage`/code `"storage"`). SQLite's actual
/// message carries trailing context after the identifier (observed:
/// `"no such column: amout in SELECT amout FROM expenses at offset 7"`) --
/// identifiers this module ever scores are never themselves `is_valid_ident`
/// (so never carry whitespace), so the first whitespace-delimited token
/// after the colon is the identifier and everything after it is discarded.
pub fn parse_bind_error(message: &str) -> Option<(&'static str, String)> {
    let first_token = |rest: &str| rest.split_whitespace().next().unwrap_or("").to_string();
    if let Some(rest) = message.split_once("no such column:") {
        return Some(("column", first_token(rest.1)));
    }
    if let Some(rest) = message.split_once("no such table:") {
        return Some(("table", first_token(rest.1)));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_thresholds_match_requirement_2() {
        assert_eq!(Band::from_similarity(1.0), Band::Covered);
        assert_eq!(Band::from_similarity(0.95), Band::FuzzyCovered);
        assert_eq!(Band::from_similarity(0.9), Band::FuzzyCovered);
        assert_eq!(Band::from_similarity(0.8), Band::Partial);
        assert_eq!(Band::from_similarity(0.75), Band::Partial);
        assert_eq!(Band::from_similarity(0.5), Band::Absent);
    }

    #[test]
    fn score_identifier_amout_matches_amount_fuzzy_covered() {
        let candidates = vec![("amount".to_string(), "amount".to_string()), ("category".to_string(), "category".to_string())];
        let diag = score_identifier("amout", "column", &candidates);
        assert_eq!(diag.matched_to.as_deref(), Some("amount"));
        assert_eq!(diag.band, Band::FuzzyCovered);
    }

    #[test]
    fn score_identifier_prefers_annotation_word_mapped_back_to_owning_column() {
        // AC7: column `amount` annotated "amount in USD"; identifier `usd`
        // must match via the tokenized annotation word, reporting the
        // owning column as matched_to, not the annotation text.
        let candidates = vec![
            ("amount".to_string(), "amount".to_string()),
            ("amount".to_string(), "usd".to_string()),
            ("amount".to_string(), "in".to_string()),
        ];
        let diag = score_identifier("usd", "column", &candidates);
        assert_eq!(diag.matched_to.as_deref(), Some("amount"));
        // The band is name-only (`usd` vs `amount` directly), not boosted
        // by the exact annotation-word match that won it the ranking --
        // see score_identifier's own doc on why "covered" must mean a
        // real identifier match, not merely a description hit.
        assert_ne!(diag.band, Band::Covered);
    }

    #[test]
    fn annotation_word_candidates_and_case_insensitive_match() {
        // AC7: `SELECT usd FROM expenses` refused; the column `amount`'s
        // description "amount in USD" must offer `amount` as a candidate
        // through the annotation text, case-insensitively (`usd` vs `USD`),
        // with a non-null hint (the band is not `Covered`: the query named
        // no real identifier, only matched a description word).
        let mut candidates: Vec<(String, String)> = vec![("amount".to_string(), "amount".to_string())];
        candidates.extend(annotation_word_candidates("amount", "amount in USD"));
        let diag = score_identifier("usd", "column", &candidates);
        assert_eq!(diag.matched_to.as_deref(), Some("amount"));
        assert_ne!(diag.band, Band::Covered, "diag: {diag:?}");
        assert!(build_hint(&diag, None).is_some(), "diag: {diag:?}");
    }

    #[test]
    fn score_value_bands_case_different_exact_as_absent() {
        let diag = score_value("Produce", &["produce".to_string(), "meat".to_string()]);
        assert_eq!(diag.band, Band::Absent);
        assert_eq!(diag.top_candidates.first().map(|c| c.name.as_str()), Some("produce"));
    }

    #[test]
    fn score_value_bands_exact_match_as_covered() {
        let diag = score_value("produce", &["produce".to_string(), "meat".to_string()]);
        assert_eq!(diag.band, Band::Covered);
        assert_eq!(diag.matched_to.as_deref(), Some("produce"));
    }

    #[test]
    fn extract_from_ast_finds_table_and_where_equality() {
        let ex = extract_from_ast("SELECT * FROM expenses WHERE category = 'Produce'");
        assert_eq!(ex.from_table.as_deref(), Some("expenses"));
        assert_eq!(ex.where_equalities, vec![("category".to_string(), "Produce".to_string())]);
    }

    #[test]
    fn extract_from_ast_unparseable_sql_returns_default() {
        let ex = extract_from_ast("not sql at all (((");
        assert!(ex.from_table.is_none());
        assert!(ex.where_equalities.is_empty());
    }

    #[test]
    fn parse_bind_error_extracts_column_and_table() {
        assert_eq!(
            parse_bind_error("no such column: amout"),
            Some(("column", "amout".to_string()))
        );
        assert_eq!(
            parse_bind_error("no such table: expense"),
            Some(("table", "expense".to_string()))
        );
        // The real SQLite/rusqlite message shape: trailing context after
        // the identifier that must be discarded, not treated as part of
        // the term.
        assert_eq!(
            parse_bind_error("no such column: amout in SELECT amout FROM expenses at offset 7"),
            Some(("column", "amout".to_string()))
        );
        assert_eq!(parse_bind_error("some other error"), None);
    }

    #[test]
    fn build_hint_names_table_and_closest_candidate() {
        let candidates = vec![("amount".to_string(), "amount".to_string())];
        let diag = score_identifier("amout", "column", &candidates);
        let hint = build_hint(&diag, Some("expenses")).expect("hint");
        assert!(hint.contains("amout"), "{hint}");
        assert!(hint.contains("expenses"), "{hint}");
        assert!(hint.contains("amount"), "{hint}");
        assert!(hint.len() <= HINT_MAX_CHARS);
    }

    #[test]
    fn build_hint_is_none_for_covered() {
        let candidates = vec![("amount".to_string(), "amount".to_string())];
        let diag = score_identifier("amount", "column", &candidates);
        assert!(build_hint(&diag, Some("expenses")).is_none());
    }
}
