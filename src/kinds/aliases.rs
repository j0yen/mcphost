//! PRD-mcphost-unknown-kind-routes-to-recipe: the kind-alias table.
//!
//! An agent builder names the job it wants ("webhook", "a cron job"), not
//! mcphost's runtime taxonomy. This table maps those job-words onto a
//! registered runtime [`crate::kinds::Kind`] plus the named `recipe` of
//! `host.*` calls that actually builds it -- `control::quickstart`/
//! `control::tool_publish` are the only two callers, both via
//! [`find_alias`].
//!
//! Requirement 1: data, not code -- `tests/kindroute_ac05_...rs` (the
//! startup test) checks every alias resolves to a kind
//! [`crate::kinds::KindRegistry::with_builtin`]-plus-extensions actually
//! registers, and every [`recipe_steps`] entry names a registered tool.

use serde_json::{Value, json};

/// One row of the alias table. `alias` is matched case-insensitively
/// (`find_alias` lowercases before comparing); `kind` is the runtime kind
/// name it resolves to; `recipe` names the [`recipe_steps`] sequence that
/// builds it.
pub struct KindAlias {
    pub alias: &'static str,
    pub kind: &'static str,
    pub recipe: &'static str,
}

/// Requirement 1: `event|events|webhook|webhooks|inbound|trigger` all name
/// the inbound-webhook recipe (PRD-mcphost-webhook-inbox); `cron|schedule|
/// scheduled` name the cron-schedule recipe (PRD-mcphost-schedules). Every
/// alias resolves to `http` today -- both recipes are `http`-kind specs
/// wired up via `host.trigger.*`, not new runtime kinds (PRD non-goal).
pub const KIND_ALIASES: &[KindAlias] = &[
    KindAlias { alias: "event", kind: "http", recipe: "webhook-inbox" },
    KindAlias { alias: "events", kind: "http", recipe: "webhook-inbox" },
    KindAlias { alias: "webhook", kind: "http", recipe: "webhook-inbox" },
    KindAlias { alias: "webhooks", kind: "http", recipe: "webhook-inbox" },
    KindAlias { alias: "inbound", kind: "http", recipe: "webhook-inbox" },
    KindAlias { alias: "trigger", kind: "http", recipe: "webhook-inbox" },
    KindAlias { alias: "cron", kind: "http", recipe: "schedules" },
    KindAlias { alias: "schedule", kind: "http", recipe: "schedules" },
    KindAlias { alias: "scheduled", kind: "http", recipe: "schedules" },
];

/// Case-insensitive alias lookup (AC2: `"EVENT"` resolves identically to
/// `"event"`).
pub fn find_alias(name: &str) -> Option<&'static KindAlias> {
    let lower = name.to_ascii_lowercase();
    KIND_ALIASES.iter().find(|a| a.alias == lower)
}

/// Every alias name, for `UnknownKind`'s `data.aliases` and
/// `host.quickstart()`'s no-kind `aliases` array.
pub fn alias_names() -> Vec<&'static str> {
    KIND_ALIASES.iter().map(|a| a.alias).collect()
}

/// Every distinct recipe name, stable order, for `host.quickstart()`'s
/// no-kind `recipes` array (AC7).
pub fn recipe_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = Vec::new();
    for a in KIND_ALIASES {
        if !names.contains(&a.recipe) {
            names.push(a.recipe);
        }
    }
    names
}

/// The ordered `host.*` calls that build `recipe`, each carrying a
/// filled-in `args_example` an agent can copy and edit (requirement 2:
/// `{name, steps: [{tool, args_example}], docs}`). `tool_name` and `spec`
/// are the same tenant-namespaced starter name and kind example
/// `control::quickstart`'s own `steps` already substitute in -- a real
/// trigger `id` doesn't exist yet at quickstart time, so the two trigger
/// steps carry an explanatory placeholder for it instead of a real one,
/// same convention as `field_example`'s placeholder `tenant_key`
/// (`errors.rs`).
pub fn recipe_steps(recipe: &str, tool_name: &str, spec: &Value) -> Vec<Value> {
    match recipe {
        "webhook-inbox" => vec![
            json!({
                "tool": "host.tool_publish",
                "args_example": {"name": tool_name, "kind": "http", "spec": spec},
            }),
            json!({
                "tool": "host.trigger.set",
                "args_example": {"tool": tool_name, "kind": "webhook", "name": tool_name},
            }),
            json!({
                "tool": "host.trigger.test",
                "args_example": {
                    "id": "<id from host.trigger.set's response>",
                    "body": {"example": true},
                },
            }),
        ],
        "schedules" => vec![
            json!({
                "tool": "host.tool_publish",
                "args_example": {"name": tool_name, "kind": "http", "spec": spec},
            }),
            json!({
                "tool": "host.trigger.set",
                "args_example": {
                    "tool": tool_name,
                    "kind": "schedule",
                    "schedule": "*/5 * * * *",
                },
            }),
            json!({
                "tool": "host.trigger.fire",
                "args_example": {"id": "<id from host.trigger.set's response>"},
            }),
        ],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alias_lookup_is_case_insensitive() {
        let a = find_alias("EVENT").expect("EVENT resolves");
        assert_eq!(a.alias, "event");
        assert_eq!(a.kind, "http");
    }

    #[test]
    fn unknown_word_has_no_alias() {
        assert!(find_alias("lambda").is_none());
    }
}
