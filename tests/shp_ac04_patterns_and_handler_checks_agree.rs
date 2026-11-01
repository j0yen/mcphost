//! PRD-mcphost-schema-shape-agreement
//! contract: reads registry handler::host_tool_descriptors
//! AC4 (P0) -- Given every `SHAPE_CHECKS` row (pattern const, handler check fn or
//! `None`, `accepts: &[&str]`), When each `SHAPE_PROBE_STRINGS` value and each row's
//! `accepts` string runs through the pattern and the check fn, Then accept/reject is
//! identical per value, every `accepts` string is accepted, and every descriptor
//! `pattern` value equals some row's const.

use crate::shape_probe::SHAPE_PROBE_STRINGS;
use crate::vbd_calls;
use mcphost::host_args::{SHAPE_CHECKS, ShapeCheck};
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// Whether `pattern` accepts `value`, by the same engine the validator uses.
fn pattern_accepts(pattern: &str, value: &str) -> bool {
    jsonschema::validator_for(&json!({"type": "string", "pattern": pattern}))
        .unwrap_or_else(|e| panic!("pattern {pattern:?} does not compile: {e}"))
        .is_valid(&json!(value))
}

/// Every `pattern` string anywhere in a schema, with its JSON pointer.
fn patterns(schema: &Value, at: &str, out: &mut Vec<(String, String)>) {
    match schema {
        Value::Object(map) => {
            for (k, v) in map {
                if k == "pattern" && let Some(p) = v.as_str() {
                    out.push((at.to_string(), p.to_string()));
                } else {
                    patterns(v, &format!("{at}/{k}"), out);
                }
            }
        }
        Value::Array(items) => items.iter().enumerate().for_each(|(i, v)| patterns(v, &format!("{at}/{i}"), out)),
        _ => {}
    }
}

fn descriptor_patterns() -> Vec<(String, String)> {
    let mut found = Vec::new();
    for tool in vbd_calls::all_descriptors() {
        let mut here = Vec::new();
        patterns(&Value::Object((*tool.input_schema).clone()), "", &mut here);
        found.extend(here.into_iter().map(|(at, p)| (format!("{} {at}", tool.name), p)));
    }
    found
}

/// Every disagreement between each row's pattern and handler check over the corpus and its `accepts`.
fn disagreements(rows: &[ShapeCheck]) -> Vec<String> {
    let mut failures = Vec::new();
    for row in rows {
        for accepted in row.accepts {
            if !pattern_accepts(row.pattern, accepted) {
                failures.push(format!("{}: pattern refuses its accepts string {accepted:?}", row.pattern));
            }
            if row.check.is_some_and(|check| !check(accepted)) {
                failures.push(format!("{}: handler check refuses its accepts string {accepted:?}", row.pattern));
            }
        }
        let Some(check) = row.check else { continue };
        for value in SHAPE_PROBE_STRINGS.iter().chain(row.accepts.iter()) {
            let (by_pattern, by_check) = (pattern_accepts(row.pattern, value), check(value));
            if by_pattern != by_check {
                failures.push(format!("{}: {value:?} is {} by the pattern but {} by the handler check", row.pattern, verdict(by_pattern), verdict(by_check)));
            }
        }
    }
    failures
}

#[test]
fn shp_ac04_pattern_and_handler_check_agree_on_the_corpus_and_each_accepts_string() {
    assert!(SHAPE_CHECKS.len() >= 10, "only {} SHAPE_CHECKS rows", SHAPE_CHECKS.len());
    let failures = disagreements(SHAPE_CHECKS);
    assert!(failures.is_empty(), "{} disagreements:\n{}", failures.len(), failures.join("\n"));
}

fn verdict(accepted: bool) -> &'static str {
    if accepted { "accepted" } else { "rejected" }
}

#[test]
fn shp_ac04_every_descriptor_pattern_is_some_rows_const_and_every_row_is_used_or_orphan() {
    let found = descriptor_patterns();
    assert!(found.len() >= 20, "only {} descriptor patterns found", found.len());
    let known: BTreeSet<&str> = SHAPE_CHECKS.iter().map(|r| r.pattern).collect();
    let unlisted: Vec<String> = found.iter().filter(|(_, p)| !known.contains(p.as_str())).map(|(at, p)| format!("{at}: {p}")).collect();
    assert!(unlisted.is_empty(), "descriptor patterns with no SHAPE_CHECKS row:\n{}", unlisted.join("\n"));
    let used: BTreeSet<&str> = found.iter().map(|(_, p)| p.as_str()).collect();
    let dead: Vec<&str> = SHAPE_CHECKS.iter().filter(|r| !r.orphan && !used.contains(r.pattern)).map(|r| r.pattern).collect();
    assert!(dead.is_empty(), "SHAPE_CHECKS rows no descriptor uses (delete them or mark `orphan: true`): {dead:?}");
    let mut seen = BTreeSet::new();
    assert!(SHAPE_CHECKS.iter().all(|r| seen.insert(r.pattern)), "a pattern appears in two SHAPE_CHECKS rows");
}

#[test]
fn shp_ac04_a_pattern_looser_than_its_check_is_named() {
    let loose = ShapeCheck { pattern: "^.+$", check: Some(mcphost::tables::is_valid_ident), accepts: &["orders", "not an ident"], orphan: false };
    let found = disagreements(&[loose]);
    assert!(found.iter().any(|f| f.contains("\"toolconf-probe\" is accepted by the pattern but rejected by the handler check")), "{found:?}");
    assert!(found.iter().any(|f| f.contains("handler check refuses its accepts string \"not an ident\"")), "{found:?}");
}
