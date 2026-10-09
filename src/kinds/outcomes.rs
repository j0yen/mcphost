//! PRD-mcphost-kind-ask-routing: the outcome map.
//!
//! An agent's `kind` argument often names an outcome ("docs", "message",
//! "database") rather than a runtime engine. This table maps that outcome
//! word to the `host.*` verbs and the shipped recipe that deliver it, so the
//! `kind '<x>' is not registered` error (`errors.rs`) and `host.quickstart`
//! (`control.rs`) can answer with the call to make instead of only the
//! engine list. Data, not code paths: `tests/kindroute_ac03_*.rs` pins every
//! verb to `tools/list` and every recipe page to `www/llms.txt`.

use serde_json::{Value, json};

/// One row of the outcome map. `words` are the normalized spellings (see
/// [`normalize`]) that select it; `verbs` are canonical `tools/list` names;
/// `page` is the `## ` heading in `www/llms.txt` that documents the recipe,
/// when one exists.
pub struct Outcome {
    pub outcome: &'static str,
    pub words: &'static [&'static str],
    pub verbs: &'static [&'static str],
    pub recipe: &'static str,
    pub page: Option<&'static str>,
    pub example_tool: &'static str,
    /// The example call's arguments, as a JSON literal.
    pub example_args: &'static str,
}

pub const OUTCOMES: &[Outcome] = &[
    Outcome {
        outcome: "docs",
        words: &["doc", "docs", "document", "documents"],
        verbs: &["host.docs.put", "host.docs.search"],
        recipe: "docs-qa",
        page: Some("Docs Q&A in a minute"),
        example_tool: "host.docs.put",
        example_args: r##"{"name": "notes.md", "content": "# Notes\nYour first document."}"##,
    },
    Outcome {
        outcome: "message",
        words: &["message", "messages", "inbox", "msg"],
        verbs: &["host.msg.send", "host.msg.inbox"],
        recipe: "agent-inbox",
        page: None,
        example_tool: "host.msg.send",
        example_args: r##"{"to": ["@handle"], "body": "hello"}"##,
    },
    Outcome {
        outcome: "database",
        words: &["database", "db", "table", "tables", "csv", "sql"],
        verbs: &["host.table.create", "host.table.append", "host.table.query"],
        recipe: "database-in-a-minute",
        page: Some("Give Claude a database in one minute"),
        example_tool: "host.table.create",
        example_args: r##"{"name": "expenses", "columns": {"id": "integer", "category": "text", "amount": "real"}, "primary_key": "id"}"##,
    },
    Outcome {
        outcome: "webhook",
        words: &["webhook", "webhooks"],
        verbs: &["host.trigger.set", "host.trigger.test"],
        recipe: "webhook-inbox",
        page: None,
        example_tool: "host.trigger.set",
        example_args: r##"{"tool": "my_tool", "kind": "webhook", "name": "my_tool"}"##,
    },
    Outcome {
        outcome: "schedule",
        words: &["cron", "schedule", "schedules", "scheduled"],
        verbs: &["host.trigger.set", "host.trigger.fire"],
        recipe: "schedules",
        page: None,
        example_tool: "host.trigger.set",
        example_args: r##"{"tool": "my_tool", "kind": "schedule", "name": "my_tool", "schedule": "*/5 * * * *"}"##,
    },
    Outcome {
        outcome: "uptime",
        words: &["uptime", "probe", "probes"],
        verbs: &["host.state.table_create", "host.state.insert"],
        recipe: "uptime-probes",
        page: Some("Uptime probes with no server"),
        example_tool: "host.state.table_create",
        example_args: r##"{"name": "targets", "schema": {"url": "text", "added_at": "real"}, "primary_key": "url"}"##,
    },
    Outcome {
        outcome: "memory",
        words: &["memory", "memories"],
        verbs: &["host.table.create", "host.table.query"],
        recipe: "team-memory",
        page: Some("Give your agents one memory"),
        example_tool: "host.table.create",
        example_args: r##"{"name": "memory", "columns": {"key": "text", "text": "text", "tags": "json", "writer": "text", "at": "real"}}"##,
    },
];

/// Every outcome word, in table order -- the words `find` matches exactly
/// (PRD-mcphost-publish-schema-from-registry: the `kind` enum on
/// `host.tool_publish` reads this same table).
pub fn all_words() -> Vec<&'static str> {
    OUTCOMES.iter().flat_map(|o| o.words.iter().copied()).collect()
}

/// Matching key for a requested `kind`: trimmed, lowercase.
pub fn normalize(word: &str) -> String {
    word.trim().to_ascii_lowercase()
}

/// The outcome whose `words` include the normalized `requested` (or that
/// word with one trailing `s` stripped, so any plural of a listed singular
/// matches), if any. Exact on the normalized word -- no fuzzy matching, so
/// the error stays deterministic.
pub fn find(requested: &str) -> Option<&'static Outcome> {
    let key = normalize(requested);
    let singular = key.strip_suffix('s').unwrap_or(&key);
    OUTCOMES
        .iter()
        .find(|o| o.words.contains(&key.as_str()) || o.words.contains(&singular))
}

impl Outcome {
    /// The `{tool, args}` example call.
    pub fn example(&self) -> Value {
        json!({
            "tool": self.example_tool,
            "args": serde_json::from_str::<Value>(self.example_args).unwrap_or(Value::Null),
        })
    }

    /// The `did_you_mean` / quickstart-recipe payload.
    pub fn to_json(&self) -> Value {
        let mut v = json!({
            "outcome": self.outcome,
            "verbs": self.verbs,
            "recipe": self.recipe,
            "example": self.example(),
        });
        if let Some(page) = self.page {
            v["page"] = json!(page);
        }
        v
    }
}

/// The URL fragment that marks an `http` tool as a hand-rolled query tool
/// over a fixture database.
const DATABASE_QUERY_PATH: &str = "/database/query/";

/// PRD-mcphost-kind-ask-routing requirement 6 (AC7): the `hint` a
/// `host.tool_publish` response carries for an `http` spec whose upstream
/// URL is a known fixture-database query path -- `csv_import` plus a
/// validated `query` already ship as `database-in-a-minute`. Looks at the
/// upstream URL in every place an `http` spec can declare one
/// (`upstream` as a bare string or `{url}`, or a top-level `url`).
pub fn publish_hint(kind: &str, spec: &Value) -> Option<&'static str> {
    if kind != "http" {
        return None;
    }
    let upstream = spec.get("upstream");
    let urls = [
        upstream.and_then(Value::as_str),
        upstream.and_then(|u| u.get("url")).and_then(Value::as_str),
        spec.get("url").and_then(Value::as_str),
    ];
    urls.into_iter()
        .flatten()
        .any(|u| u.contains(DATABASE_QUERY_PATH))
        .then_some("database-in-a-minute")
}
