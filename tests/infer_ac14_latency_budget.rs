//! PRD-mcphost-tool-infer AC14 (P0) — Given a source of 200 lines, When it
//! is published, Then inference adds no more than 100ms to the publish
//! call.
//!
//! Measured directly against the pure inference functions rather than a
//! full HTTP round trip: an end-to-end publish call's wall time is
//! dominated by HTTP/tokio/sandboxed-AST-subprocess overhead that has
//! nothing to do with this PRD's inference step, so it isn't a fair way to
//! isolate "what inference itself adds". `mcphost::kinds::infer`'s
//! functions are the entire cost inference contributes to a publish call
//! (see that module's docs on why `describe`/`call` recompute rather than
//! reuse a stored value) -- timing them directly is the precise test of
//! requirement 12's budget.

use mcphost::kinds::infer::{infer_python_args_schema, infer_python_requirements};
use std::time::Instant;

fn generate_200_line_source() -> String {
    let mut src = String::from("import json\nimport os\n\ndef main(args):\n");
    for i in 0..90 {
        src.push_str(&format!(
            "    v{i} = args.get(\"key_{i}\", \"default_{i}\")\n"
        ));
    }
    src.push_str("    required = args[\"the_required_key\"]\n");
    while src.lines().count() < 200 {
        src.push_str("    # padding line to reach 200 lines\n");
    }
    src.push_str("    return {\"ok\": True}\n");
    src
}

#[test]
fn inference_over_a_200_line_source_is_well_under_100ms() {
    let source = generate_200_line_source();
    assert!(source.lines().count() >= 200, "fixture must be >=200 lines");

    let started = Instant::now();
    let schema = infer_python_args_schema(&source).expect("schema inference ok");
    let requirements = infer_python_requirements(&source).expect("requirements inference ok");
    let elapsed = started.elapsed();

    assert!(
        elapsed.as_millis() < 100,
        "inference over a 200-line source took {elapsed:?}, over the 100ms budget"
    );
    assert_eq!(schema["required"], serde_json::json!(["the_required_key"]));
    assert!(requirements.is_empty(), "json/os are both stdlib");
}
