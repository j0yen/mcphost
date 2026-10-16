//! PRD-mcphost-event-kinds-registry-generated AC2: `schema_enum()`,
//! `/events.json`, `render_events_section()`, `docs/EVENTS.md` and
//! `www/event-kinds.json` each carry exactly the `EventKind::ALL` strings, in
//! order, and nothing else.

use crate::common;
use common::TestServer;
use mcphost::events::{EventKind, QueueState};
use mcphost::llms_txt::render_events_section;

fn all() -> Vec<String> {
    EventKind::ALL.iter().map(|k| k.as_str().to_string()).collect()
}

fn strings(v: &serde_json::Value) -> Vec<String> {
    v.as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect()
}

/// The first backticked cell of each row of the markdown table that follows `heading`.
fn md_table_first_cells(doc: &str, heading: &str) -> Vec<String> {
    let after = doc.split(heading).nth(1).expect("heading present");
    after
        .lines()
        .skip_while(|l| !l.starts_with('|'))
        .skip(2)
        .take_while(|l| l.starts_with('|'))
        .map(|l| l.split('|').nth(1).unwrap().trim().trim_matches('`').to_string())
        .collect()
}

#[tokio::test]
async fn evkr_ac02() {
    let expected = all();
    assert_eq!(expected.len(), 10);

    // schema_enum()
    let schema = mcphost::events::schema_enum();
    assert_eq!(schema["type"], "string");
    assert_eq!(strings(&schema["enum"]), expected);

    // /events.json
    let server = TestServer::start().await;
    let resp = reqwest::get(format!("{}/events.json", server.base_url)).await.expect("GET /events.json");
    assert_eq!(resp.headers()["cache-control"], "public, max-age=300");
    let body: serde_json::Value = resp.json().await.expect("json body");
    assert_eq!(body["generated_from"], "src/events.rs");
    let served: Vec<String> =
        body["kinds"].as_array().unwrap().iter().map(|k| k["kind"].as_str().unwrap().to_string()).collect();
    assert_eq!(served, expected);
    let states: Vec<String> =
        body["queue_states"].as_array().unwrap().iter().map(|s| s["state"].as_str().unwrap().to_string()).collect();
    assert_eq!(states, QueueState::ALL.iter().map(|s| s.as_str().to_string()).collect::<Vec<_>>());
    assert_eq!(body, mcphost::events::events_json());

    // render_events_section(): one `- kind — ...` line per kind, nothing else bulleted
    let section = render_events_section();
    let lines: Vec<String> = section
        .lines()
        .filter_map(|l| l.strip_prefix("- "))
        .map(|l| l.split(" — ").next().unwrap().to_string())
        .collect();
    assert_eq!(lines, expected);

    // docs/EVENTS.md
    let root = env!("CARGO_MANIFEST_DIR");
    let doc = std::fs::read_to_string(format!("{root}/docs/EVENTS.md")).unwrap();
    assert_eq!(md_table_first_cells(&doc, "## Event kinds"), expected);
    assert_eq!(doc, mcphost::events::render_events_doc(), "docs/EVENTS.md drifted: run `mcphost gen-docs`");

    // www/event-kinds.json
    let file = std::fs::read_to_string(format!("{root}/www/event-kinds.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&file).unwrap();
    assert_eq!(strings(&parsed["kinds"]), expected);
    assert_eq!(parsed.as_object().unwrap().len(), 2);
    assert_eq!(file, mcphost::events::render_event_kinds_json(), "www/event-kinds.json drifted");

    // www/llms.txt carries the same section
    let llms = std::fs::read_to_string(format!("{root}/www/llms.txt")).unwrap();
    assert!(llms.contains(&section), "www/llms.txt events section drifted: run `mcphost llms-txt`");
}
