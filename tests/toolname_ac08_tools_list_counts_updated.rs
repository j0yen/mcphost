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
/// this PRD landed.
const PRE_PRD_TOTAL: usize = 147;

#[test]
fn full_registry_grew_by_exactly_the_alias_count_with_nothing_removed() {
    let kinds = KindRegistry::with_builtin();
    let total = host_tool_descriptors(&kinds).len();
    assert_eq!(
        total,
        PRE_PRD_TOTAL + TOOL_ALIASES.len(),
        "the full host.*/billing.* registry must have grown by exactly the number of aliases \
         this PRD added, with nothing removed"
    );
    assert_eq!(total, 167, "pin the exact new total compat_ac04_ac05's own count was updated to");
}
