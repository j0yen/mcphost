//! PRD-mcphost-chain-publish-time-path-check AC7 (P0) -- Given
//! `CONTEXT_KEYS`, When the parity test builds a context and reads the
//! checker's accepted keys, Then both equal the table and a scan of
//! `src/kinds/chain.rs` finds no string literal `"prev"`, `"steps"`, or
//! `"result"` outside the table and its tests.

use mcphost::kinds::chain::{CONTEXT_KEYS, accepted_prev_keys, accepted_step_keys, step_context};
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn keys(v: &Value) -> BTreeSet<String> {
    v.as_object().expect("object").keys().cloned().collect()
}

#[test]
fn constructor_and_checker_both_read_the_table() {
    let sample = json!({"a": 1});
    let ctx = step_context(&json!({}), Some(&sample), &[sample.clone(), sample.clone()]);

    let top: BTreeSet<String> =
        [CONTEXT_KEYS.input, CONTEXT_KEYS.prev, CONTEXT_KEYS.steps, CONTEXT_KEYS.event].map(String::from).into();
    assert_eq!(keys(&ctx), top, "constructor's top-level keys must be the table's");

    let under: BTreeSet<String> = [CONTEXT_KEYS.result.to_string()].into();
    assert_eq!(keys(&ctx[CONTEXT_KEYS.prev]), under);
    assert_eq!(accepted_prev_keys(), under, "checker's $.prev keys must be the table's");
    assert_eq!(accepted_step_keys(), under, "checker's $.steps[i] keys must be the table's");
    for step in ctx[CONTEXT_KEYS.steps].as_array().expect("steps array") {
        assert_eq!(keys(step), under);
    }
}

#[test]
fn chain_rs_spells_the_context_keys_only_in_the_table_and_its_tests() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/kinds/chain.rs");
    let text = std::fs::read_to_string(path).expect("read src/kinds/chain.rs");
    let begin = text.find("// CONTEXT_KEYS-BEGIN").expect("table start marker");
    let end = text.find("// CONTEXT_KEYS-END").expect("table end marker");
    let tests_at = text.find("#[cfg(test)]\nmod tests").expect("tests module");
    assert!(begin < end && end < tests_at);

    let mut offenders = Vec::new();
    for (n, line) in text[..begin].lines().chain(text[end..tests_at].lines()).enumerate() {
        if line.trim_start().starts_with("//") {
            continue;
        }
        for lit in ["\"prev\"", "\"steps\"", "\"result\""] {
            if line.contains(lit) {
                offenders.push(format!("{n}: {}", line.trim()));
            }
        }
    }
    // The chain SPEC's own `steps` field is a different contract:
    // scripts/spec-fields-doc-check.sh parses `known_spec_fields()`'s literal
    // straight out of the file, so that one line stays spelled out.
    offenders.retain(|o| !o.ends_with("&[\"steps\"]"));
    // The dry-run (PRD-mcphost-chain-dry-run) writes its own report entry
    // (`"result": fixture`) and its fixture-error `data` (`"steps": <count>`);
    // those are output fields, not keys of the path-resolution context.
    offenders.retain(|o| {
        !(o.ends_with("json!({\"key\": key, \"steps\": steps.len()}),")
            || o.ends_with("json!({\"key\": key, \"steps\": matches}),")
            || o.ends_with("\"result\": fixture,"))
    });
    assert!(offenders.is_empty(), "context keys spelled outside the table: {offenders:#?}");
}
