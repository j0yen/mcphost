//! PRD-mcphost-tool-naming-convention-and-aliases
//! AC8 — Given the existing test suite, When any test asserted an exact
//! `tools/list` count, Then it is updated to the new count with a
//! comment citing this PRD, and no other test changed.
//!
//! `tests/compat_ac04_ac05_admin_and_tenant_cache_fields.rs` (147 -> 167)
//! was the one pre-existing exact-count assertion this PRD's registry
//! growth (+20 aliases, 0 removals) affected; its own fix carries a
//! comment citing this PRD. This file pins the same new total as its own
//! regression test, read straight off the live registry rather than a
//! second hand-copied literal.

use mcphost::handler::host_tool_descriptors;
use mcphost::kinds::KindRegistry;
use mcphost::tool_aliases::TOOL_ALIASES;

/// The pre-PRD `host.*`/`billing.*` control-plane total
/// `compat_ac04_ac05_admin_and_tenant_cache_fields.rs` asserted before
/// this PRD landed, carried forward through this PRD's rebase onto main
/// (which had grown 147 -> 150 with host.table.graph/join_paths/
/// next_questions, then 150 -> 151 with host.table.query_log from
/// PRD-mcphost-table-context-and-sql-passthrough, then 151 -> 155 with
/// host.drift.reviews/review/resolve/check from PRD-mcphost-drift-review,
/// then 155 -> 157 with host.table.query_diagnose/query_stats from
/// PRD-mcphost-query-diagnosis) -- same total compat_ac04_ac05's own
/// comment pins.
const PRE_PRD_TOTAL: usize = 157;

/// PRD-mcphost-invite-links added host.invite.create/list/revoke (+3, no
/// aliases) after this PRD landed.
const POST_PRD_GROWTH: usize = 3;

/// r368-prep rebase (2026-10-03): PRD-mcphost-result-handles lands after
/// this PRD in tree order and adds three genuinely new, non-alias tools
/// (host.table.handles/handle_drop/handle_export) -- same kind of later-
/// PRD growth compat_ac04_ac05_admin_and_tenant_cache_fields.rs's own
/// comment already accounts for across multiple rebasing PRDs.
const POST_PRD_NON_ALIAS_GROWTH: usize = 3;

#[test]
fn full_registry_grew_by_exactly_the_alias_count_with_nothing_removed() {
    let kinds = KindRegistry::with_builtin();
    let total = host_tool_descriptors(&kinds).len();
    assert_eq!(
        total,
        PRE_PRD_TOTAL + TOOL_ALIASES.len() + POST_PRD_GROWTH + POST_PRD_NON_ALIAS_GROWTH,
        "the full host.*/billing.* registry must have grown by exactly the number of aliases \
         this PRD added (plus any later-landing PRD's own non-alias tools, and any earlier- \
         or later-landing PRD's own documented growth), with nothing removed"
    );
    assert_eq!(total, 183, "pin the exact new total PRD-mcphost-invite-links and this PRD's own growth grew it to");
}
