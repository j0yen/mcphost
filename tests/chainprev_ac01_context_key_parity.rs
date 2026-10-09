//! PRD-mcphost-chain-prev-contract AC1 (P0) -- Given the step-context
//! constructor, When the parity test builds a context and reads its keys
//! under `prev` and each `steps[i]`, Then the set equals the checker's
//! accepted first-level keys (today exactly `{result}`), and the test fails
//! if either side changes alone.

use mcphost::kinds::chain::{accepted_prev_keys, accepted_step_keys, step_context};
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn keys(v: &Value) -> BTreeSet<String> {
    v.as_object().expect("object").keys().cloned().collect()
}

#[test]
fn checker_accepted_keys_equal_the_built_contexts_keys() {
    let sample = json!({"a": 1});
    let ctx = step_context(&json!({}), Some(&sample), &[sample.clone(), sample.clone()]);

    let built_prev = keys(&ctx["prev"]);
    assert_eq!(built_prev, accepted_prev_keys(), "checker and constructor disagree under $.prev");
    for (i, step) in ctx["steps"].as_array().expect("steps array").iter().enumerate() {
        assert_eq!(
            keys(step),
            accepted_step_keys(),
            "checker and constructor disagree under $.steps[{i}]"
        );
    }

    // Pin today's grammar: a change to the constructor alone trips this
    // literal, a change to the checker alone trips the equalities above.
    let only_result: BTreeSet<String> = ["result".to_string()].into();
    assert_eq!(built_prev, only_result);
    assert_eq!(accepted_prev_keys(), only_result);
    assert_eq!(accepted_step_keys(), only_result);
}
