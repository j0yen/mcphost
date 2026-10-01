//! `emit(&recommendation, &rows)` -- an inline-data Vega-Lite v5 spec from
//! a [`Recommendation`] and the query's own rows (a `serde_json::Value`
//! array, read generically rather than through
//! `mqo_result_profiler::ResultProfile`, since a spec's `data.values` is
//! just the caller's rows verbatim). `BigNumber` (a `text` mark centered on
//! the one measure) and `Table` (no mark-specific encoding; the rows speak
//! for themselves) are special-cased -- neither is a real Vega-Lite mark
//! name, so both degrade to `"text"` for the spec's own `mark` field while
//! `host.table.chart`'s own `recommendation.mark` (not this spec) is what a
//! caller reads to tell them apart.

use mqo_chart_recommender::Recommendation;
use mqo_chart_vocab::Mark;
use serde_json::{Map, Value, json};

pub const VEGA_LITE_SCHEMA_URL: &str = "https://vega.github.io/schema/vega-lite/v5.json";

fn vega_mark_str(mark: Mark) -> &'static str {
    match mark {
        Mark::Bar => "bar",
        Mark::Line => "line",
        Mark::Point => "point",
        Mark::Area => "area",
        // Vega-Lite has no literal "bignumber"/"table" mark; both render as
        // plain text (a single centered value, or the raw rows via
        // `data.values` alone) -- see this module's own doc comment.
        Mark::BigNumber | Mark::Table => "text",
    }
}

fn encoding_field(field: &mqo_chart_recommender::EncodingField) -> Value {
    json!({"field": field.field, "type": field.type_})
}

/// requirement 3: an inline-data (`data.values` holds `rows` verbatim, no
/// external `data.url`) Vega-Lite v5 spec for `recommendation`.
pub fn emit(recommendation: &Recommendation, rows: &Value) -> Value {
    let values = rows.as_array().cloned().unwrap_or_default();
    let mut spec = Map::new();
    spec.insert("$schema".to_string(), json!(VEGA_LITE_SCHEMA_URL));
    spec.insert("data".to_string(), json!({"values": values}));
    spec.insert("mark".to_string(), json!(vega_mark_str(recommendation.mark)));

    let mut encoding = Map::new();
    if let Some(x) = &recommendation.encoding.x {
        encoding.insert("x".to_string(), encoding_field(x));
    }
    if let Some(y) = &recommendation.encoding.y {
        encoding.insert("y".to_string(), encoding_field(y));
    }
    if let Some(text) = &recommendation.encoding.text {
        encoding.insert("text".to_string(), encoding_field(text));
    }
    if !encoding.is_empty() {
        spec.insert("encoding".to_string(), Value::Object(encoding));
    }

    Value::Object(spec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mqo_chart_recommender::{Encoding, EncodingField};

    fn field(name: &str, type_: &str) -> EncodingField {
        EncodingField { field: name.to_string(), type_: type_.to_string() }
    }

    #[test]
    fn bar_spec_has_inline_data_and_nominal_quantitative_encoding() {
        let rec = Recommendation {
            mark: Mark::Bar,
            encoding: Encoding { x: Some(field("category", "nominal")), y: Some(field("total", "quantitative")), text: None },
            rationale: "r".to_string(),
            alternatives: vec![],
        };
        let rows = json!([{"category": "produce", "total": 100.0}]);
        let spec = emit(&rec, &rows);
        assert_eq!(spec["$schema"], VEGA_LITE_SCHEMA_URL);
        assert_eq!(spec["mark"], "bar");
        assert_eq!(spec["data"]["values"], rows);
        assert_eq!(spec["encoding"]["x"]["type"], "nominal");
        assert_eq!(spec["encoding"]["y"]["type"], "quantitative");
    }

    #[test]
    fn line_spec_encodes_x_as_temporal() {
        let rec = Recommendation {
            mark: Mark::Line,
            encoding: Encoding { x: Some(field("day", "temporal")), y: Some(field("total", "quantitative")), text: None },
            rationale: "r".to_string(),
            alternatives: vec![],
        };
        let spec = emit(&rec, &json!([]));
        assert_eq!(spec["mark"], "line");
        assert_eq!(spec["encoding"]["x"]["type"], "temporal");
    }

    #[test]
    fn bignumber_spec_uses_text_mark_and_text_encoding() {
        let rec = Recommendation {
            mark: Mark::BigNumber,
            encoding: Encoding { x: None, y: None, text: Some(field("count", "quantitative")) },
            rationale: "r".to_string(),
            alternatives: vec![Mark::Table],
        };
        let rows = json!([{"count": 1000}]);
        let spec = emit(&rec, &rows);
        assert_eq!(spec["mark"], "text");
        assert_eq!(spec["encoding"]["text"]["field"], "count");
        assert_eq!(spec["data"]["values"], rows);
    }

    #[test]
    fn table_spec_has_no_encoding_and_empty_data_for_empty_rows() {
        let rec = Recommendation {
            mark: Mark::Table,
            encoding: Encoding::default(),
            rationale: "r".to_string(),
            alternatives: vec![],
        };
        let spec = emit(&rec, &json!([]));
        assert_eq!(spec["mark"], "text");
        assert!(spec.get("encoding").is_none(), "spec: {spec}");
        assert_eq!(spec["data"]["values"], json!([]));
    }

    #[test]
    fn non_array_rows_degrade_to_empty_values_rather_than_panicking() {
        let rec = Recommendation {
            mark: Mark::Table,
            encoding: Encoding::default(),
            rationale: "r".to_string(),
            alternatives: vec![],
        };
        let spec = emit(&rec, &Value::Null);
        assert_eq!(spec["data"]["values"], json!([]));
    }
}
