//! AC6 (P1) — Given the validator restriction analysis, When assignment
//! expressions are executor-safe, Then the recorded spec validates and
//! executes; otherwise the README spec-language section documents the
//! restriction and why.
//!
//! Determination (see README.md's "Python spec-language notes" and
//! `kinds::python::enhance_syntax_message`'s doc comment): assignment
//! expressions ARE executor-safe in general (mcphost adds no restriction
//! of its own -- both the publish-time AST check and the tool's own
//! runtime compile `source` with the same CPython grammar). The one
//! rejection that exists (`obj.attr := 1` / `d[k] := 1`) is CPython's own
//! grammar restricting assignment-expression *targets* to plain names --
//! genuinely executor-unsafe, not a validator-only restriction to relax.
//! So this AC's second branch applies: the README documents the
//! restriction and why. This test is the "test" that AC pairs with (per
//! this repo's `--derive` convention, a documentation AC can pair with an
//! assertion against the doc's own content rather than runtime behavior).

use std::fs;
use std::path::Path;

#[test]
fn readme_documents_the_assignment_expression_target_restriction() {
    let readme_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md");
    let readme = fs::read_to_string(&readme_path).expect("README.md must exist");

    assert!(
        readme.contains("Python spec-language notes"),
        "README must have a spec-language section for the python kind"
    );
    assert!(
        readme.contains("assignment expression"),
        "README must name assignment expressions"
    );
    // Names the restriction is the language's own, not mcphost's.
    assert!(
        readme.contains("CPython") || readme.contains("Python's own grammar"),
        "README must explain the restriction is executor-level, not validator-added"
    );
    // Names the accepted alternative.
    assert!(
        readme.contains("plain name") && readme.contains("separate"),
        "README must name the accepted alternative"
    );
}
