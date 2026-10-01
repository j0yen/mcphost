//! `generate_caption(&CaptionInput, &CaptionConfig)` -- a headline and up
//! to three facts whose values are computed from the rows, never guessed
//! by a model. `CaptionInput` carries the typed
//! [`mqo_result_profiler::ResultProfile`] (not a raw `Value`, unlike
//! `mqo-chart-recommender`/`mqo-vega-emitter`) because picking which column
//! is the measure to summarize needs the profile's own `role` field, not a
//! second ad hoc lookup.

use mqo_chart_vocab::Role;
use mqo_result_profiler::ResultProfile;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// requirement 9 (P2): how [`generate_caption`]'s numbers would be
/// formatted for a given locale -- thousands separator and currency
/// symbol. Carried on [`CaptionConfig`] per the vendored shape; not
/// exercised by any of this PRD's P0/P1 acceptance criteria (all of which
/// use `CaptionConfig::default`'s en-US policy), so `generate_caption`
/// reads only the default policy today.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormatPolicy {
    pub thousands_separator: String,
    pub currency_symbol: String,
}

impl Default for FormatPolicy {
    fn default() -> Self {
        FormatPolicy { thousands_separator: ",".to_string(), currency_symbol: "$".to_string() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptionConfig {
    pub locale: String,
    pub format_policy: FormatPolicy,
}

impl Default for CaptionConfig {
    fn default() -> Self {
        CaptionConfig { locale: "en-US".to_string(), format_policy: FormatPolicy::default() }
    }
}

/// [`generate_caption`]'s input: the query result's own profile and rows.
pub struct CaptionInput {
    pub profile: ResultProfile,
    pub rows: Vec<Value>,
}

/// One caption fact: a label, a value recomputed from `rows` (never a
/// model's own arithmetic), and a provenance string naming how it was
/// computed -- `host.table.chart` passes this through unchanged (technical
/// considerations).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fact {
    pub label: String,
    pub value: Value,
    pub provenance: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Caption {
    pub headline: String,
    pub facts: Vec<Fact>,
}

/// requirement 4/AC5: the exact headline a zero-row (or otherwise
/// unsummarizable) result gets -- a fixed string, not a template with
/// interpolated zeros, so a caller can match on it directly.
pub const NO_TAKEAWAY_HEADLINE: &str = "No rows matched this query; there is no takeaway to caption.";

fn format_number(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 {
        format!("{}", v.round() as i64)
    } else {
        format!("{v:.2}")
    }
}

fn value_label(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn measure_value(row: &Value, measure_name: &str) -> f64 {
    row.get(measure_name).and_then(Value::as_f64).unwrap_or(0.0)
}

/// requirement 2/AC3/AC6: every number in the caption is recomputed from
/// `input.rows` right here -- never copied from a model's own guess.
pub fn generate_caption(input: &CaptionInput, _config: &CaptionConfig) -> Caption {
    if input.rows.is_empty() {
        return Caption { headline: NO_TAKEAWAY_HEADLINE.to_string(), facts: Vec::new() };
    }

    let Some(measure) = input.profile.columns.iter().find(|c| c.role == Role::Measure) else {
        return Caption { headline: NO_TAKEAWAY_HEADLINE.to_string(), facts: Vec::new() };
    };
    let dimension = input.profile.columns.iter().find(|c| c.role == Role::Dimension);

    // AC6: a single row with no dimension (`BigNumber`) -- the headline
    // states the measure's own value.
    if dimension.is_none() && input.rows.len() == 1 {
        let raw_value = input.rows[0].get(&measure.name).cloned().unwrap_or(Value::Null);
        let v = measure_value(&input.rows[0], &measure.name);
        return Caption {
            headline: format!("{} is {}", measure.label, format_number(v)),
            facts: vec![Fact {
                label: measure.label.clone(),
                value: raw_value,
                provenance: format!("{} from the query result's only row", measure.name),
            }],
        };
    }

    let Some(dimension) = dimension else {
        let v = measure_value(&input.rows[0], &measure.name);
        return Caption {
            headline: format!("{} is {}", measure.label, format_number(v)),
            facts: vec![Fact {
                label: measure.label.clone(),
                value: serde_json::json!(v),
                provenance: format!("{} from the query result's first row", measure.name),
            }],
        };
    };

    // AC3: one dimension, one measure, multiple rows -- the headline names
    // the row with the largest measure value; facts recompute the max,
    // total, and row count from `input.rows`.
    let mut top_label = String::new();
    let mut top_value = f64::NEG_INFINITY;
    let mut total = 0.0;
    for row in &input.rows {
        let v = measure_value(row, &measure.name);
        total += v;
        if v > top_value {
            top_value = v;
            top_label = row.get(&dimension.name).map(value_label).unwrap_or_default();
        }
    }
    let count = input.rows.len() as i64;

    Caption {
        headline: format!(
            "{top_label} is the largest {} by {} at {}",
            dimension.label,
            measure.label,
            format_number(top_value)
        ),
        facts: vec![
            Fact {
                label: format!("largest {}", measure.label),
                value: serde_json::json!(top_value),
                provenance: format!("max({}) over {count} rows", measure.name),
            },
            Fact {
                label: format!("total {}", measure.label),
                value: serde_json::json!(total),
                provenance: format!("sum({}) over {count} rows", measure.name),
            },
            Fact {
                label: format!("{} count", dimension.label),
                value: serde_json::json!(count),
                provenance: format!("count of rows grouped by {}", dimension.name),
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mqo_chart_vocab::DataType;
    use mqo_result_profiler::{ColumnProfile, MeasureRange};
    use serde_json::json;

    fn dim_col(name: &str) -> ColumnProfile {
        ColumnProfile {
            name: name.to_string(),
            label: name.to_string(),
            role: Role::Dimension,
            data_type: DataType::Nominal,
            cardinality: 3,
            null_rate: 0.0,
            measure_range: None,
            is_temporal: false,
        }
    }

    fn measure_col(name: &str) -> ColumnProfile {
        ColumnProfile {
            name: name.to_string(),
            label: name.to_string(),
            role: Role::Measure,
            data_type: DataType::Quantitative,
            cardinality: 3,
            null_rate: 0.0,
            measure_range: Some(MeasureRange { min: 0.0, max: 100.0 }),
            is_temporal: false,
        }
    }

    #[test]
    fn empty_rows_get_the_no_takeaway_headline_and_no_facts() {
        let input = CaptionInput {
            profile: ResultProfile::new(0, vec![]),
            rows: vec![],
        };
        let caption = generate_caption(&input, &CaptionConfig::default());
        assert_eq!(caption.headline, NO_TAKEAWAY_HEADLINE);
        assert!(caption.facts.is_empty());
    }

    #[test]
    fn single_row_no_dimension_states_the_value_in_the_headline() {
        let input = CaptionInput {
            profile: ResultProfile::new(1, vec![measure_col("COUNT(*)")]),
            rows: vec![json!({"COUNT(*)": 1000})],
        };
        let caption = generate_caption(&input, &CaptionConfig::default());
        assert!(caption.headline.contains("1000"), "headline: {}", caption.headline);
        assert_eq!(caption.facts.len(), 1);
        assert_eq!(caption.facts[0].value, json!(1000));
    }

    #[test]
    fn dimension_and_measure_names_the_largest_group_and_recomputes_facts() {
        let input = CaptionInput {
            profile: ResultProfile::new(3, vec![dim_col("category"), measure_col("total")]),
            rows: vec![
                json!({"category": "produce", "total": 200.0}),
                json!({"category": "bakery", "total": 500.0}),
                json!({"category": "dairy", "total": 300.0}),
            ],
        };
        let caption = generate_caption(&input, &CaptionConfig::default());
        assert!(caption.headline.contains("bakery"), "headline: {}", caption.headline);
        assert_eq!(caption.facts.len(), 3);
        assert_eq!(caption.facts[0].value, json!(500.0));
        assert_eq!(caption.facts[1].value, json!(1000.0));
        assert_eq!(caption.facts[2].value, json!(3));
    }

    #[test]
    fn provenance_is_carried_on_every_fact() {
        let input = CaptionInput {
            profile: ResultProfile::new(1, vec![dim_col("category"), measure_col("total")]),
            rows: vec![json!({"category": "produce", "total": 200.0})],
        };
        let caption = generate_caption(&input, &CaptionConfig::default());
        assert!(caption.facts.iter().all(|f| !f.provenance.is_empty()));
    }

    #[test]
    fn config_default_is_en_us() {
        let config = CaptionConfig::default();
        assert_eq!(config.locale, "en-US");
        assert_eq!(config.format_policy.currency_symbol, "$");
    }
}
