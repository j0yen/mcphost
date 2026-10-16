//! PRD-mcphost-event-kinds-registry-generated AC3: a variant added to
//! `EventKind` without a `description()`/`data_fields()` arm fails
//! `cargo test`. `events.rs` writes both as exhaustive `match`es with no `_`
//! arm, so a missing arm is a compile error; this test additionally refuses an
//! empty description or field list, and a variant missing from `ALL`, naming
//! the variant.

use mcphost::events::EventKind;

/// Exhaustive on purpose (no `_` arm): adding a variant breaks the build here
/// until it is counted, which is what makes the `ALL` length check below bite.
fn variant_count_marker(k: EventKind) -> usize {
    match k {
        EventKind::RunDone
        | EventKind::RunFailed
        | EventKind::RunProgress
        | EventKind::InboxMessage
        | EventKind::TableChanged
        | EventKind::StateChanged
        | EventKind::QueueDrained
        | EventKind::QueueDeadLetter
        | EventKind::TriggerFired
        | EventKind::ToolPublished => 1,
    }
}

fn problems(kinds: &[EventKind]) -> Vec<String> {
    let mut out = Vec::new();
    for k in kinds {
        if k.description().trim().is_empty() {
            out.push(format!("event kind {} has no description", k.as_str()));
        }
        if k.data_fields().is_empty() {
            out.push(format!("event kind {} has no data_fields", k.as_str()));
        }
    }
    out
}

#[test]
fn evkr_every_kind_has_description_and_fields() {
    let p = problems(EventKind::ALL);
    assert!(p.is_empty(), "{}", p.join("; "));
    let total: usize = EventKind::ALL.iter().map(|k| variant_count_marker(*k)).sum();
    assert_eq!(total, 10, "EventKind::ALL must list every variant exactly once");
    let mut names: Vec<&str> = EventKind::ALL.iter().map(|k| k.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), EventKind::ALL.len(), "duplicate wire string in EventKind::ALL");
}
