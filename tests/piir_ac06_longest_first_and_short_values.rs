//! PRD-mcphost-pii-tagging-and-log-redaction-v2
//! AC6 -- Given two tagged values where one contains the other, When redacted,
//! Then the longer is replaced first and no partial value survives; Given a
//! value of 2 characters, Then it stays intact.

use mcphost::calltrace::Redactor;
use mcphost::logsink::{LogSink, PiiSet, REDACTION};
use serde_json::json;
use std::collections::HashMap;

fn set(args: serde_json::Value) -> PiiSet {
    let names: Vec<String> = args.as_object().expect("object").keys().cloned().collect();
    PiiSet::from_args(&names, &args)
}

#[test]
fn piir_ac06_the_longer_value_is_replaced_first() {
    // `john` sits inside `john@x.io`: replacing the short one first would leave `@x.io`.
    let pii = set(json!({"name": "john", "email": "john@x.io"}));
    assert_eq!(pii.redact("mail john@x.io (john)"), format!("mail {REDACTION} ({REDACTION})"));
    // The same through the call trace's redactor, which the request, the tails and the error use.
    let redactor = Redactor::new(&HashMap::new()).with_pii(&pii);
    let out = redactor.text("mail john@x.io (john)");
    assert_eq!(out, format!("mail {REDACTION} ({REDACTION})"));
    for partial in ["john", "@x.io", "x.io"] {
        assert!(!out.contains(partial), "no partial value survives: {out}");
    }

    // A value inside the middle of another, and one that is a suffix.
    let pii = set(json!({"host": "example.com", "email": "alice@example.com"}));
    assert_eq!(pii.redact("alice@example.com via example.com"), format!("{REDACTION} via {REDACTION}"));
    let pii = set(json!({"a": "abc", "b": "xabcx"}));
    assert_eq!(pii.redact("xabcx abc"), format!("{REDACTION} {REDACTION}"));

    // Percent-encoded forms follow the same order.
    let pii = set(json!({"name": "john", "email": "john@x.io"}));
    assert_eq!(pii.redact("q=john%40x.io&n=john"), format!("q={REDACTION}&n={REDACTION}"));

    // Redacting twice changes nothing, even where a value occurs inside the marker.
    let pii = set(json!({"v": "pii"}));
    let once = pii.redact("a pii b");
    assert_eq!(once, format!("a {REDACTION} b"));
    assert_eq!(pii.redact(&once), once);
}

#[test]
fn piir_ac06_a_two_character_value_stays_intact() {
    let pii = set(json!({"state": "NY", "zip": 90, "city": "NYC"}));
    // Two characters, and a number of fewer than three digits: never needles.
    let text = "state NY zip 90 city NYC";
    assert_eq!(pii.redact(text), format!("state NY zip 90 city {REDACTION}"));
    let redactor = Redactor::new(&HashMap::new()).with_pii(&pii);
    assert_eq!(redactor.text(text), format!("state NY zip 90 city {REDACTION}"));
    // Three digits is the floor.
    assert_eq!(set(json!({"n": 123})).redact("n=123 n=12"), format!("n={REDACTION} n=12"));
}

#[tokio::test]
async fn piir_ac06_the_sink_write_applies_the_same_order() {
    mcphost::logsink::scoped(async {
        mcphost::logsink::arm(
            &["name".to_string(), "email".to_string(), "state".to_string()],
            &json!({"name": "john", "email": "john@x.io", "state": "NY"}),
        );
        let written = LogSink::ToolLog.write_through("john@x.io john NY");
        assert_eq!(written, format!("{REDACTION} {REDACTION} NY"));
    })
    .await;
}
