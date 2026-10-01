//! `result-profile.v1` types -- the typed inventory `mqo-chart-recommender`
//! reads (as JSON, see that crate) and `mqo-chart-caption` reads directly
//! (as this crate's own [`ResultProfile`]). Upstream (ai-stack), this crate
//! also builds a profile from an MQO response; mcphost vendors the TYPES
//! only (PRD-mcphost-chart-in-a-minute requirement 1's "for the
//! `ResultProfile` types only") -- `src/chart.rs` builds a [`ResultProfile`]
//! from a `host.table.query` result's own columns and rows instead, since
//! mcphost has no MQO response to profile.

use mqo_chart_vocab::{DataType, Role};
use serde::{Deserialize, Serialize};

/// A measure column's observed numeric range. `None` on a
/// [`ColumnProfile`] whose `role` is [`Role::Dimension`] -- a dimension's
/// range isn't a meaningful chart input.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MeasureRange {
    pub min: f64,
    pub max: f64,
}

/// One column of a `result-profile.v1` value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColumnProfile {
    pub name: String,
    pub label: String,
    pub role: Role,
    pub data_type: DataType,
    pub cardinality: i64,
    pub null_rate: f64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub measure_range: Option<MeasureRange>,
    pub is_temporal: bool,
}

/// A query result's own typed shape: one row-count/measure-count/
/// dimension-count summary plus one [`ColumnProfile`] per returned column.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResultProfile {
    pub schema: String,
    pub row_count: i64,
    pub measure_count: i64,
    pub dimension_count: i64,
    pub columns: Vec<ColumnProfile>,
}

impl ResultProfile {
    /// This crate's own schema tag -- every [`ResultProfile`] this crate
    /// constructs carries it, so a consumer (`mqo-chart-recommender`,
    /// reading the JSON form) can tell a genuine profile from an arbitrary
    /// JSON object.
    pub const SCHEMA: &'static str = "result-profile.v1";

    /// Builds a profile from `row_count` and `columns`, deriving
    /// `measure_count`/`dimension_count` from each column's own `role`
    /// rather than asking the caller to keep them in sync by hand.
    pub fn new(row_count: i64, columns: Vec<ColumnProfile>) -> Self {
        let measure_count = columns.iter().filter(|c| c.role == Role::Measure).count() as i64;
        let dimension_count = columns.iter().filter(|c| c.role == Role::Dimension).count() as i64;
        ResultProfile {
            schema: Self::SCHEMA.to_string(),
            row_count,
            measure_count,
            dimension_count,
            columns,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measure(name: &str) -> ColumnProfile {
        ColumnProfile {
            name: name.to_string(),
            label: name.to_string(),
            role: Role::Measure,
            data_type: DataType::Quantitative,
            cardinality: 5,
            null_rate: 0.0,
            measure_range: Some(MeasureRange { min: 0.0, max: 10.0 }),
            is_temporal: false,
        }
    }

    fn dimension(name: &str, data_type: DataType) -> ColumnProfile {
        ColumnProfile {
            name: name.to_string(),
            label: name.to_string(),
            role: Role::Dimension,
            data_type,
            cardinality: 5,
            null_rate: 0.0,
            measure_range: None,
            is_temporal: data_type == DataType::Temporal,
        }
    }

    #[test]
    fn new_derives_measure_and_dimension_counts() {
        let profile = ResultProfile::new(
            5,
            vec![dimension("category", DataType::Nominal), measure("total")],
        );
        assert_eq!(profile.schema, ResultProfile::SCHEMA);
        assert_eq!(profile.row_count, 5);
        assert_eq!(profile.measure_count, 1);
        assert_eq!(profile.dimension_count, 1);
    }

    #[test]
    fn new_with_no_columns_counts_zero() {
        let profile = ResultProfile::new(0, vec![]);
        assert_eq!(profile.measure_count, 0);
        assert_eq!(profile.dimension_count, 0);
    }

    #[test]
    fn measure_range_omitted_from_json_for_dimensions() {
        let dim = dimension("day", DataType::Temporal);
        let json = serde_json::to_value(&dim).expect("serialize");
        assert!(json.get("measure_range").is_none(), "json: {json}");
        assert_eq!(json["is_temporal"], serde_json::json!(true));
    }

    #[test]
    fn column_profile_round_trips_through_json() {
        let col = measure("total");
        let json = serde_json::to_string(&col).expect("serialize");
        let back: ColumnProfile = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, col);
    }

    #[test]
    fn result_profile_round_trips_through_json() {
        let profile = ResultProfile::new(1, vec![measure("count")]);
        let json = serde_json::to_string(&profile).expect("serialize");
        let back: ResultProfile = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, profile);
    }
}
