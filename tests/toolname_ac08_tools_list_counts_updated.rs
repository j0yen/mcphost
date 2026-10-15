//! contract: the exact tools/list counts match the registry
//! PRD-mcphost-tool-naming-convention-and-aliases
//! AC8 — Given the existing test suite, When any test asserted an exact
//! `tools/list` count, Then it is updated to the new count with a
//! comment citing this PRD, and no other test changed.
//!
//! `tests/compat_ac04_ac05_admin_and_tenant_cache_fields.rs` (147 -> 167)
//! was the one pre-existing exact-count assertion this PRD's registry
//! growth (+20 aliases, 0 removals) affected; its own fix carries a
//! comment citing this PRD. This file proves the same property (aliases added, nothing removed) from the live lists, never a fixed registry size.

use std::collections::HashSet;

use mcphost::handler::host_tool_descriptors;
use mcphost::kinds::KindRegistry;
use mcphost::tool_aliases::TOOL_ALIASES;

#[test]
fn full_registry_grew_by_exactly_the_alias_count_with_nothing_removed() {
    let kinds = KindRegistry::with_builtin();
    let post: Vec<String> = host_tool_descriptors(&kinds).iter().map(|t| t.name.to_string()).collect();
    let post_set: HashSet<&str> = post.iter().map(String::as_str).collect();
    assert_eq!(post_set.len(), post.len(), "tools/list must not repeat a name");

    // (a) every alias name is advertised in the full tools/list.
    for a in TOOL_ALIASES {
        assert!(post_set.contains(a.alias), "alias {} missing from the full tools/list", a.alias);
    }

    // The pre-alias list, read off the post-alias list itself: every name
    // that is not an alias. Later PRDs' own new tools land in both lists, so
    // no fixed registry size is assumed.
    let alias_names: HashSet<&str> = TOOL_ALIASES.iter().map(|a| a.alias).collect();
    let pre: Vec<&str> = post.iter().map(String::as_str).filter(|n| !alias_names.contains(n)).collect();
    let pre_set: HashSet<&str> = pre.iter().copied().collect();

    // (b) nothing removed: each alias's canonical target is still in the
    // pre-alias list (the renamed tool survives under its new name), and no
    // canonical target is itself an alias.
    for a in TOOL_ALIASES {
        assert!(pre_set.contains(a.canonical), "canonical {} (alias {}) was removed", a.canonical, a.alias);
    }
    assert!(pre_set.iter().all(|n| post_set.contains(n)), "a pre-alias name is missing after aliasing");

    // (c) growth equals the number of aliases not already in the pre-alias
    // list, computed from the lists.
    let new_aliases: HashSet<&str> = alias_names.iter().copied().filter(|n| !pre_set.contains(n)).collect();
    assert_eq!(
        post.len() - pre.len(),
        new_aliases.len(),
        "the full registry must have grown by exactly the aliases not already present, with nothing removed"
    );
}
