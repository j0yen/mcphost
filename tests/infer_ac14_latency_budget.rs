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
//!
//! PRD-mcphost-tests-host-independence requirement 1: a fixed 100ms budget
//! is a RedBaron fact, not a property of this code -- the first clean
//! RedBaron/box parity (2026-09-11) measured 139ms against a load average
//! of 98 and failed here while passing on the idle box. The budget now
//! scales with `host::load_multiplier()` (`load1 / nproc`, floored at
//! `1.0`), read from `/proc/loadavg` fresh at test start, so a contended
//! host gets proportionally more room while an idle host keeps the
//! original, un-widened 100ms bound -- the property this AC actually
//! guards (inference itself stays cheap) never gets weaker, only the
//! wall-clock allowance for host noise changes.

#[path = "support/host.rs"]
#[allow(dead_code)]
mod host;

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

    // Requirement 1's own technical note (this file is not guaranteed to
    // run first in its binary once the gate's 4 test threads reorder
    // things): warm the codegen/allocator paths once, unmeasured, so a
    // cold first-call cost is never billed to the timed run below,
    // regardless of what order the test harness actually picks.
    let _ = infer_python_args_schema(&source);
    let _ = infer_python_requirements(&source);

    let multiplier = host::load_multiplier();
    let budget = std::time::Duration::from_micros((100_000.0 * multiplier) as u64);

    let started = Instant::now();
    let schema = infer_python_args_schema(&source).expect("schema inference ok");
    let requirements = infer_python_requirements(&source).expect("requirements inference ok");
    let elapsed = started.elapsed();

    assert!(
        elapsed <= budget,
        "inference over a 200-line source took {elapsed:?}, over the load-scaled budget of \
         {budget:?} (100ms x {multiplier:.2} load multiplier); {}",
        host::describe_host(),
    );
    assert_eq!(schema["required"], serde_json::json!(["the_required_key"]));
    assert!(requirements.is_empty(), "json/os are both stdlib");
}
