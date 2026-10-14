//! PRD-mcphost-publish-time-tool-lint
//! AC5 (P0) -- Given a line `rows = mcphost.state.query(table="t")["rows"]`,
//! When linted, Then `string_arg_subscripted` does not fire and the run-488
//! envelope rule, when present, may.

use mcphost::kinds::python_lint::lint_source;
use serde_json::{Value, json};

fn rules(findings: &[Value]) -> Vec<&str> {
    findings.iter().map(|f| f["rule"].as_str().unwrap()).collect()
}

#[test]
fn envelope_subscript_is_not_a_string_argument_subscript() {
    // `q` is a string-typed argument (default ""), so rule 3 is armed; the
    // subscripted value is an mcphost envelope, not an argument.
    let source = "import mcphost\n\n\ndef main(args):\n    q = args.get(\"q\", \"\")\n    rows = mcphost.state.query(table=\"t\")[\"rows\"]\n    return {\"q\": q, \"rows\": rows}\n";
    let findings = lint_source(source, None);
    assert!(!rules(&findings).contains(&"string_arg_subscripted"), "{findings:?}");
    assert!(
        findings.iter().all(|f| f["line"] != json!(6)),
        "no rule currently fires on the envelope line: {findings:?}"
    );

    // Also with the envelope bound to a name that a string argument shadows
    // nowhere: subscripting it later stays clean.
    let bound = "import mcphost\n\n\ndef main(args):\n    q = args.get(\"q\", \"\")\n    res = mcphost.state.query(table=\"t\")\n    return {\"n\": res[\"rows\"], \"q\": q}\n";
    assert_eq!(lint_source(bound, None), Vec::<Value>::new());

    // And rule 3 still fires on the argument itself in the same file.
    let both = "import mcphost\n\n\ndef main(args):\n    q = args.get(\"q\", \"\")\n    rows = mcphost.state.query(table=\"t\")[\"rows\"]\n    return {\"v\": q[\"v\"], \"rows\": rows}\n";
    let findings = lint_source(both, None);
    assert_eq!(rules(&findings), vec!["string_arg_subscripted"]);
    assert_eq!(findings[0]["line"], json!(7));
}
