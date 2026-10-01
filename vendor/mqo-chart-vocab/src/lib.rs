//! Shared, grammar-free vocabulary the rest of the ai-stack chart chain
//! (`mqo-chart-recommender`, `mqo-vega-emitter`, `mqo-chart-caption`,
//! `mqo-result-profiler`) builds on: what a chart's mark can be, how a
//! column's data is typed for encoding purposes, and whether a column is a
//! measure or a dimension. Depends only on `serde` -- every other crate in
//! the chain either depends on this one or on plain `serde_json::Value`, so
//! a mark or data-type name is spelled exactly one way across the whole
//! chain.

use serde::{Deserialize, Serialize};

/// A chart's rendering kind. Deliberately does not include `pie` (or any
/// other mark not already backed by a Vega-Lite encoding this chain knows
/// how to emit) -- a caller asking for one gets a validation error naming
/// the marks that are actually allowed for that result shape, not a
/// silently-ignored request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mark {
    Bar,
    Line,
    Point,
    Area,
    BigNumber,
    Table,
}

impl Mark {
    /// Every mark this chain can recommend or accept as an override,
    /// lowest-surprise-first. Used by tests that want to enumerate the
    /// whole vocabulary rather than one recommendation's own alternatives.
    pub const ALL: [Mark; 6] =
        [Mark::Bar, Mark::Line, Mark::Point, Mark::Area, Mark::BigNumber, Mark::Table];

    pub fn as_str(self) -> &'static str {
        match self {
            Mark::Bar => "bar",
            Mark::Line => "line",
            Mark::Point => "point",
            Mark::Area => "area",
            Mark::BigNumber => "bignumber",
            Mark::Table => "table",
        }
    }

    /// Case-insensitive parse of [`Self::as_str`]'s own spelling. `None`
    /// for anything else (including `"pie"`) -- there is no mark this chain
    /// doesn't already know how to name.
    pub fn parse(s: &str) -> Option<Mark> {
        match s.to_ascii_lowercase().as_str() {
            "bar" => Some(Mark::Bar),
            "line" => Some(Mark::Line),
            "point" => Some(Mark::Point),
            "area" => Some(Mark::Area),
            "bignumber" | "big_number" | "big-number" => Some(Mark::BigNumber),
            "table" => Some(Mark::Table),
            _ => None,
        }
    }
}

impl std::fmt::Display for Mark {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How a column's values should be encoded -- the Vega-Lite `type` a
/// column's field maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DataType {
    Quantitative,
    Temporal,
    Nominal,
    Ordinal,
}

impl DataType {
    pub fn as_str(self) -> &'static str {
        match self {
            DataType::Quantitative => "quantitative",
            DataType::Temporal => "temporal",
            DataType::Nominal => "nominal",
            DataType::Ordinal => "ordinal",
        }
    }
}

impl std::fmt::Display for DataType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Whether a `result-profile.v1` column is a number to aggregate/plot
/// (`Measure`) or a value to group/label by (`Dimension`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Measure,
    Dimension,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Measure => "measure",
            Role::Dimension => "dimension",
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_parse_round_trips_every_variant_case_insensitively() {
        for mark in Mark::ALL {
            let s = mark.as_str();
            assert_eq!(Mark::parse(s), Some(mark), "lowercase round trip for {s}");
            assert_eq!(Mark::parse(&s.to_ascii_uppercase()), Some(mark), "uppercase round trip for {s}");
        }
    }

    #[test]
    fn mark_parse_rejects_pie_and_unknown_strings() {
        assert_eq!(Mark::parse("pie"), None);
        assert_eq!(Mark::parse("scatter3d"), None);
        assert_eq!(Mark::parse(""), None);
    }

    #[test]
    fn mark_serde_uses_lowercase_strings() {
        let json = serde_json::to_string(&Mark::BigNumber).expect("serialize");
        assert_eq!(json, "\"bignumber\"");
        let back: Mark = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, Mark::BigNumber);
    }

    #[test]
    fn data_type_and_role_display_match_as_str() {
        assert_eq!(DataType::Temporal.to_string(), "temporal");
        assert_eq!(Role::Measure.to_string(), "measure");
        assert_eq!(Role::Dimension.as_str(), "dimension");
    }

    #[test]
    fn mark_all_has_no_duplicates() {
        let mut seen = std::collections::HashSet::new();
        for mark in Mark::ALL {
            assert!(seen.insert(mark), "duplicate mark in Mark::ALL: {mark}");
        }
        assert_eq!(seen.len(), Mark::ALL.len());
    }
}
