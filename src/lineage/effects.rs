//! PRD-mcphost-lineage-blast-radius requirement 2: the effect table, ported
//! from ai-stack's `effect_for` and re-keyed for mcphost's own consumer
//! kinds. Data in one place (this one `match`), exhaustively covering every
//! `(ChangeKind, NodeKind)` pair -- AC5 asserts every pair returns a
//! defined `(effect, severity, action)` and that `Add` is `Severity::None`
//! for every consumer.

use super::graph::{ChangeKind, NodeKind, Severity};

/// requirement 2: `effect_for(change_kind, consumer_kind) -> (Effect,
/// Severity, Action)`. `effect` is a short, change-kind-level description
/// of what happened (the same string for every consumer of one change
/// kind); `severity` is the per-consumer ranking bucket `blast_radius`
/// sorts on; `action` is the remediation `host.table.drop`'s
/// `lineage_blocked` and `host.lineage.blast_radius` both surface verbatim.
pub fn effect_for(change: ChangeKind, consumer: NodeKind) -> (&'static str, Severity, &'static str) {
    use ChangeKind::*;
    use NodeKind::*;
    use Severity::*;

    // requirement 2 / AC5: "Add is none for every consumer" -- checked
    // first so it can never be shadowed by a later, more specific arm.
    if matches!(change, Add) {
        return ("added", None, "no action needed");
    }

    match (change, consumer) {
        (Drop, Table) => ("removed", Severity::None, "no action needed"),
        (Drop, Run) => ("removed", Severity::None, "no action needed; past runs are historical records"),
        (Drop, Column) => ("removed", Breaking, "the column no longer exists once its table is dropped"),
        (Drop, Tool) => ("removed", Breaking, "remove or rewrite the tool's query"),
        (Drop, Chain) => ("removed", Breaking, "remove or rewrite the chain step that reads this table"),
        (Drop, Document) => ("removed", Breaking, "the document's source table is gone; re-derive or delete it"),
        (Drop, Chart) => ("removed", Breaking, "delete or repoint the chart"),
        (Drop, Handle) => ("removed", Breaking, "invalidate the handle"),

        (Rename, Table) => ("renamed", Severity::None, "no action needed"),
        (Rename, Run) => ("renamed", Severity::None, "no action needed; past runs are historical records"),
        (Rename, Column) => ("renamed", Breaking, "the column's table name changed"),
        (Rename, Tool) => ("renamed", Breaking, "update the tool's query to the new table name"),
        (Rename, Chain) => ("renamed", Breaking, "update the chain step's query to the new table name"),
        (Rename, Document) => ("renamed", Breaking, "update the document's derived_from reference"),
        (Rename, Chart) => ("renamed", Breaking, "repoint the chart to the new table name"),
        (Rename, Handle) => ("renamed", Breaking, "re-materialize the handle under the new name"),

        (TypeChange, Table) => ("type_changed", Severity::None, "no action needed"),
        (TypeChange, Run) => ("type_changed", Severity::None, "no action needed; past runs are historical records"),
        (TypeChange, Column) => ("type_changed", Severity::None, "no action needed"),
        (TypeChange, Tool) => ("type_changed", Breaking, "update the tool's query/parsing for the new type"),
        (TypeChange, Chain) => ("type_changed", Breaking, "update the chain step's query/parsing for the new type"),
        (TypeChange, Document) => ("type_changed", Degrading, "verify the document's description still matches the new type"),
        (TypeChange, Chart) => ("type_changed", Degrading, "verify the chart still renders with the new type"),
        (TypeChange, Handle) => ("type_changed", Degrading, "verify the handle still renders with the new type"),

        (Remove, Table) => ("column_removed", Severity::None, "no action needed"),
        (Remove, Run) => ("column_removed", Severity::None, "no action needed; past runs are historical records"),
        (Remove, Column) => ("column_removed", Severity::None, "no action needed"),
        (Remove, Tool) => ("column_removed", Breaking, "update the tool's query; a column it read is gone"),
        (Remove, Chain) => ("column_removed", Breaking, "update the chain step's query; a column it read is gone"),
        (Remove, Document) => ("column_removed", Degrading, "verify the document's description still matches the remaining columns"),
        (Remove, Chart) => ("column_removed", Degrading, "verify the chart still renders without the removed column"),
        (Remove, Handle) => ("column_removed", Degrading, "verify the handle still renders without the removed column"),

        (Add, _) => unreachable!("Add is handled above"),
    }
}
