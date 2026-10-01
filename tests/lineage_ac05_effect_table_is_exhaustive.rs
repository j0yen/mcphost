//! PRD-mcphost-lineage-blast-radius AC5 (P0) -- Given the effect table, When every `(ChangeKind,
//! consumer_kind)` pair is evaluated in a test, Then each returns a
//! defined effect, severity, and action, and `Add` is `none` for every
//! consumer.

use mcphost::lineage::graph::{ChangeKind, NodeKind, Severity};
use mcphost::lineage::effects::effect_for;

#[test]
fn every_change_kind_and_consumer_kind_pair_has_a_defined_effect() {
    for change in ChangeKind::all() {
        for consumer in NodeKind::all() {
            let (effect, severity, action) = effect_for(change, consumer);
            assert!(!effect.is_empty(), "{change:?}/{consumer:?}: effect must be non-empty");
            assert!(!action.is_empty(), "{change:?}/{consumer:?}: action must be non-empty");
            let _: Severity = severity;

            if matches!(change, ChangeKind::Add) {
                assert_eq!(
                    severity,
                    Severity::None,
                    "{change:?}/{consumer:?}: Add must be none for every consumer"
                );
            }
        }
    }
}
