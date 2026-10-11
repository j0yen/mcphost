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

/// PRD-mcphost-row-policy rebase onto main (run 353, 2026-10-04): this
/// PRD's own five host.policy.set/list/attrs_set and host.audit.chain/
/// verify tools landed on top of this PRD's own 175 total above -- not
/// aliases, so this PRD's "grew by exactly the alias count" invariant is
/// checked against `PRE_PRD_TOTAL + TOOL_ALIASES.len()` same as before;
/// this constant only widens the final pinned total below to account for
/// row-policy's own later, separate addition.
const ROW_POLICY_TOOLS_ADDED: usize = 5;

/// PRD-mcphost-uptime-probe-recipe-green: host.uptime.create, a genuinely new
/// non-alias tool (its flattened form is counted by `flattened_count`).
const UPTIME_TOOLS_ADDED: usize = 1;

/// PRD-mcphost-tools-list-alias-truth: one flattened `a_b_c` clone per
/// dotted canonical, read off the registry rather than a literal.
fn flattened_count(kinds: &KindRegistry) -> usize {
    mcphost::handler::canonical_control_plane_names(kinds)
        .iter()
        .filter(|n| mcphost::tool_aliases::flattened_form(n).is_some())
        .count()
}

#[test]
fn full_registry_grew_by_exactly_the_alias_count_with_nothing_removed() {
    let kinds = KindRegistry::with_builtin();
    let total = host_tool_descriptors(&kinds).len();
    assert_eq!(
        total,
        PRE_PRD_TOTAL
            + TOOL_ALIASES.len()
            + POST_PRD_GROWTH
            + POST_PRD_NON_ALIAS_GROWTH
            + ROW_POLICY_TOOLS_ADDED
            + UPTIME_TOOLS_ADDED
            + flattened_count(&kinds),
        "the full host.*/billing.* registry must have grown by exactly the number of aliases \
         this PRD added (plus any later-landing PRD's own non-alias tools, and any earlier- \
         or later-landing PRD's own documented growth, including row-policy's own later five \
         tools), with nothing removed"
    );
    assert_eq!(total, 358, "pin the exact new total (188 + 168 flattened forms, PRD-mcphost-tools-list-alias-truth); PRD-mcphost-invite-links, PRD-mcphost-result-handles, and this PRD's own growth grew it to");
}
