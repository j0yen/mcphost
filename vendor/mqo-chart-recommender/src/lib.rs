//! `recommend(&Value)` -- picks a [`Mark`] and an [`Encoding`] from a
//! `result-profile.v1` inventory (a plain `serde_json::Value`, the same
//! shape [`mqo_result_profiler::ResultProfile`] serializes to; this crate
//! reads it generically rather than depending on that crate's types, so it
//! stays usable against any producer of that schema). Grammar-free: no
//! query language, just the profile's own `row_count` and each column's
//! `role`/`data_type`.

use mqo_chart_vocab::Mark;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One encoding channel: the field it reads and the Vega-Lite `type` it
/// should be declared with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncodingField {
    pub field: String,
    #[serde(rename = "type")]
    pub type_: String,
}

/// The channels [`emit`] (in `mqo-vega-emitter`) reads to build a spec's
/// `encoding` block. Every field is optional because not every mark uses
/// every channel (`BigNumber` uses only `text`; `Table` uses none).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Encoding {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub x: Option<EncodingField>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub y: Option<EncodingField>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub text: Option<EncodingField>,
}

/// [`recommend`]'s own output: the chosen mark, its encoding, a one-line
/// human-readable rationale, and the other marks a caller may legally
/// request instead (`host.table.chart`'s `mark` override argument is
/// validated against `{mark} ∪ alternatives`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recommendation {
    pub mark: Mark,
    pub encoding: Encoding,
    pub rationale: String,
    pub alternatives: Vec<Mark>,
}

fn columns_with_role<'a>(columns: &'a [Value], role: &str) -> Vec<&'a Value> {
    columns.iter().filter(|c| c.get("role").and_then(Value::as_str) == Some(role)).collect()
}

fn field_name(column: &Value) -> String {
    column.get("name").and_then(Value::as_str).unwrap_or_default().to_string()
}

fn is_temporal(column: &Value) -> bool {
    column.get("data_type").and_then(Value::as_str) == Some("temporal")
}

fn encoding_of(column: &Value, type_: &str) -> EncodingField {
    EncodingField { field: field_name(column), type_: type_.to_string() }
}

/// requirement 3/4: picks a mark from the profile's own shape --
/// zero rows -> `Table`; exactly one row with exactly one measure ->
/// `BigNumber`; exactly one dimension and one measure -> `Line` when the
/// dimension is temporal, else `Bar`; anything else -> `Table` (the
/// always-safe fallback, since every `result-profile.v1` value can be
/// rendered as a table of its own rows).
pub fn recommend(profile: &Value) -> Recommendation {
    let row_count = profile.get("row_count").and_then(Value::as_i64).unwrap_or(0);
    let columns: Vec<Value> =
        profile.get("columns").and_then(Value::as_array).cloned().unwrap_or_default();

    let dimensions = columns_with_role(&columns, "dimension");
    let measures = columns_with_role(&columns, "measure");

    if row_count == 0 {
        return Recommendation {
            mark: Mark::Table,
            encoding: Encoding::default(),
            rationale: "the query returned no rows".to_string(),
            alternatives: Vec::new(),
        };
    }

    if row_count == 1 && measures.len() == 1 {
        return Recommendation {
            mark: Mark::BigNumber,
            encoding: Encoding { text: Some(encoding_of(measures[0], "quantitative")), ..Encoding::default() },
            rationale: "a single row with a single measure is a headline number".to_string(),
            alternatives: vec![Mark::Table],
        };
    }

    if dimensions.len() == 1 && measures.len() == 1 {
        let dim = dimensions[0];
        let measure = measures[0];
        let x = Some(encoding_of(dim, if is_temporal(dim) { "temporal" } else { "nominal" }));
        let y = Some(encoding_of(measure, "quantitative"));
        if is_temporal(dim) {
            return Recommendation {
                mark: Mark::Line,
                encoding: Encoding { x, y, text: None },
                rationale: "one temporal dimension and one measure trace a trend over time".to_string(),
                alternatives: vec![Mark::Bar, Mark::Point, Mark::Table],
            };
        }
        return Recommendation {
            mark: Mark::Bar,
            encoding: Encoding { x, y, text: None },
            rationale: "one dimension and one measure compare categories".to_string(),
            alternatives: vec![Mark::Line, Mark::Point, Mark::Table],
        };
    }

    Recommendation {
        mark: Mark::Table,
        encoding: Encoding::default(),
        rationale: "no bar/line/bignumber shape matched this result; a table always fits".to_string(),
        alternatives: vec![Mark::Bar],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn dim(name: &str, data_type: &str) -> Value {
        json!({"name": name, "label": name, "role": "dimension", "data_type": data_type})
    }

    fn measure(name: &str) -> Value {
        json!({"name": name, "label": name, "role": "measure", "data_type": "quantitative"})
    }

    fn profile(row_count: i64, columns: Vec<Value>) -> Value {
        json!({"schema": "result-profile.v1", "row_count": row_count, "columns": columns})
    }

    #[test]
    fn zero_rows_recommends_table_with_no_alternatives() {
        let rec = recommend(&profile(0, vec![]));
        assert_eq!(rec.mark, Mark::Table);
        assert!(rec.alternatives.is_empty());
    }

    #[test]
    fn single_row_single_measure_recommends_bignumber() {
        let rec = recommend(&profile(1, vec![measure("count")]));
        assert_eq!(rec.mark, Mark::BigNumber);
        assert_eq!(rec.encoding.text.as_ref().unwrap().field, "count");
        assert_eq!(rec.alternatives, vec![Mark::Table]);
    }

    #[test]
    fn one_nominal_dimension_and_one_measure_recommends_bar() {
        let rec = recommend(&profile(5, vec![dim("category", "nominal"), measure("total")]));
        assert_eq!(rec.mark, Mark::Bar);
        assert_eq!(rec.encoding.x.as_ref().unwrap().field, "category");
        assert_eq!(rec.encoding.x.as_ref().unwrap().type_, "nominal");
        assert_eq!(rec.encoding.y.as_ref().unwrap().field, "total");
        assert!(!rec.alternatives.contains(&Mark::BigNumber));
    }

    #[test]
    fn one_temporal_dimension_and_one_measure_recommends_line() {
        let rec = recommend(&profile(20, vec![dim("day", "temporal"), measure("total")]));
        assert_eq!(rec.mark, Mark::Line);
        assert_eq!(rec.encoding.x.as_ref().unwrap().type_, "temporal");
    }

    #[test]
    fn no_alternatives_ever_include_pie() {
        for (row_count, columns) in [
            (5i64, vec![dim("category", "nominal"), measure("total")]),
            (1, vec![measure("count")]),
            (5, vec![dim("day", "temporal"), measure("total")]),
            (5, vec![measure("a"), measure("b")]),
        ] {
            let rec = recommend(&profile(row_count, columns));
            assert!(
                Mark::parse("pie").is_none()
                    && !rec.alternatives.iter().any(|m| m.as_str() == "pie")
                    && rec.mark.as_str() != "pie",
                "pie leaked into a recommendation: {rec:?}"
            );
        }
    }

    #[test]
    fn unmatched_shape_falls_back_to_table() {
        let rec = recommend(&profile(5, vec![measure("a"), measure("b")]));
        assert_eq!(rec.mark, Mark::Table);
    }
}
