//! PRD-mcphost-paged-trait-on-every-list-verb
//! AC1 (P0) -- Given the contract at HEAD, When the census counts list-shaped
//! canonical verbs, those with `cursor`, and distinct end-of-list conventions
//! in the suite's recorded responses, Then it writes them to
//! `target/pgtr-census.json` and fails if the cursored share is already 100 %.
//!
//! "List-shaped" is measured, not asserted: a canonical verb is list-shaped
//! when its recorded response is a pure list (one array payload, at most a
//! cursor field), or its input schema already declares `cursor`. The PRD's
//! baseline figures (32 list-shaped, 10 cursored, 3 conventions) are pinned
//! exactly and written beside the measured ones.

use crate::common;
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// How a recorded response says "this was the last page".
fn end_of_list_convention(response: &Value) -> Option<&'static str> {
    let obj = response.as_object()?;
    if !obj.values().any(Value::is_array) {
        return None;
    }
    match obj.get("next_cursor").or_else(|| obj.get("cursor")) {
        None => Some("absent"),
        Some(Value::Null) => Some("null_when_short"),
        Some(_) => Some("always_present"),
    }
}

/// A pure list response: exactly one array payload, plus at most a cursor.
/// Records that merely carry an incidental array (`tags`, `shared_tools`,
/// `allowlist`) beside scalar fields are not list verbs.
fn is_pure_list(response: &Value) -> bool {
    let Some(obj) = response.as_object() else {
        return false;
    };
    let payload = obj.iter().filter(|(k, _)| !matches!(k.as_str(), "cursor" | "next_cursor"));
    let (arrays, others): (Vec<_>, Vec<_>) = payload.partition(|(_, v)| v.is_array());
    arrays.len() == 1 && others.is_empty()
}

#[tokio::test]
async fn census_counts_list_verbs_cursors_and_end_of_list_conventions() {
    let tools = common::pgtr_canonical_contract_tools();
    let recorded = common::pgtr_record_responses().await;

    let cursored: BTreeSet<String> = tools
        .iter()
        .filter(|t| t["input_schema"]["properties"].get("cursor").is_some())
        .map(|t| t["name"].as_str().unwrap_or_default().to_string())
        .collect();

    let mut list_shaped: BTreeSet<String> = cursored.clone();
    let mut conventions: BTreeSet<&'static str> = BTreeSet::new();
    for (verb, response) in &recorded {
        if is_pure_list(response) {
            list_shaped.insert(verb.clone());
        }
        if cursored.contains(verb)
            && let Some(c) = end_of_list_convention(response)
        {
            conventions.insert(c);
        }
    }

    let census = json!({
        "list_shaped": list_shaped.len(),
        "cursored": cursored.len(),
        "end_of_list_conventions": conventions.len(),
        "conventions": conventions,
        "cursored_verbs": cursored,
        "prd_baseline": {"list_shaped": 32, "cursored": 10, "end_of_list_conventions": 3},
    });
    let target = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
    std::fs::create_dir_all(&target).expect("target dir");
    std::fs::write(
        target.join("pgtr-census.json"),
        serde_json::to_string_pretty(&census).expect("census json") + "\n",
    )
    .expect("write census");

    assert_eq!(cursored.len(), 10, "cursored verbs drifted from the PRD baseline: {cursored:?}");
    assert!(
        cursored.len() < list_shaped.len(),
        "cursored share is already 100% ({} of {}): the PRD has nothing left to do",
        cursored.len(),
        list_shaped.len()
    );
    assert_eq!(
        list_shaped.len(),
        32,
        "list-shaped verbs drifted from the PRD baseline 32: {list_shaped:?}"
    );
    assert_eq!(
        conventions,
        BTreeSet::from(["absent", "always_present", "null_when_short"]),
        "end-of-list conventions drifted from the PRD's 3"
    );
}
