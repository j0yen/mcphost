//! PRD-mcphost-kind-ask-routing
//! AC3 (P0) -- Given the outcome map test, When run, Then every verb it
//! names is in tools/list and every recipe page it names exists in
//! `llms.txt` (served from `www/llms.txt`).

use crate::common;
use common::{TempDataDir, all_kinds_registry};
use mcphost::kinds::outcomes::OUTCOMES;

const LLMS_TXT: &str = include_str!("../www/llms.txt");

#[test]
fn every_outcome_verb_is_in_tools_list_and_every_page_is_in_llms_txt() {
    let dir = TempDataDir::new();
    let kinds = all_kinds_registry(&dir.0);
    let live = mcphost::llms_txt::tenant_tool_names(&kinds);

    assert!(OUTCOMES.len() >= 7, "map must cover the PRD's outcome words");
    for o in OUTCOMES {
        assert!(!o.verbs.is_empty(), "outcome {} names no verbs", o.outcome);
        for verb in o.verbs.iter().chain(std::iter::once(&o.example_tool)) {
            assert!(
                live.iter().any(|t| t == verb),
                "outcome '{}' names {verb}, which is not in tools/list",
                o.outcome
            );
        }
        if let Some(page) = o.page {
            let heading = format!("## {page}\n");
            assert!(
                LLMS_TXT.contains(&heading),
                "outcome '{}' names page '{page}', which is not a section of llms.txt",
                o.outcome
            );
        }
        assert!(
            serde_json::from_str::<serde_json::Value>(o.example_args).is_ok_and(|v| v.is_object()),
            "outcome '{}' example args must be a JSON object",
            o.outcome
        );
    }
    // The PRD's required words are all present.
    for word in [
        "docs", "document", "documents", "message", "messages", "inbox", "database", "table",
        "csv", "webhook", "cron", "schedule", "uptime", "memory",
    ] {
        assert!(mcphost::kinds::outcomes::find(word).is_some(), "no outcome for '{word}'");
    }
}
