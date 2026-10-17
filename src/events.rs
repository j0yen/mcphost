//! PRD-mcphost-event-kinds-registry-generated: one [`EventKind`] enum and one
//! [`QueueState`] enum are the only place an event kind or queue state is
//! spelled. The JSON-schema `enum` of every tool argument that names a kind
//! ([`schema_enum`]), `GET /events.json` ([`events_json`]), the llms.txt
//! `## Events` section, `docs/EVENTS.md` and `www/event-kinds.json` are all
//! rendered from [`EventKind::ALL`]; the drift tests in
//! `tests/evkr_ac*.rs` assert every rendering equals the enum.
//!
//! `description()` and `data_fields()` are exhaustive `match`es with no `_`
//! arm, so a variant added without a line there does not compile; the
//! `evkr_every_kind_has_description_and_fields` test additionally names a
//! variant whose text is empty.

use serde_json::{Value, json};

use crate::errors::AppError;

/// Every event kind the host names on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    RunDone,
    RunFailed,
    RunProgress,
    InboxMessage,
    TableChanged,
    StateChanged,
    QueueDrained,
    QueueDeadLetter,
    TriggerFired,
    ToolPublished,
}

impl EventKind {
    /// The one list every consumer reads, in rendering order.
    pub const ALL: &'static [EventKind] = &[
        EventKind::RunDone,
        EventKind::RunFailed,
        EventKind::RunProgress,
        EventKind::InboxMessage,
        EventKind::TableChanged,
        EventKind::StateChanged,
        EventKind::QueueDrained,
        EventKind::QueueDeadLetter,
        EventKind::TriggerFired,
        EventKind::ToolPublished,
    ];

    /// The wire string -- the only place it is spelled.
    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::RunDone => "run.done",
            EventKind::RunFailed => "run.failed",
            EventKind::RunProgress => "run.progress",
            EventKind::InboxMessage => "inbox.message",
            EventKind::TableChanged => "table.changed",
            EventKind::StateChanged => "state.changed",
            EventKind::QueueDrained => "queue.drained",
            EventKind::QueueDeadLetter => "queue.dead_letter",
            EventKind::TriggerFired => "trigger.fired",
            EventKind::ToolPublished => "tool.published",
        }
    }

    /// One line a receiver's author reads. No `_` arm: a new variant must add one.
    pub fn description(self) -> &'static str {
        match self {
            EventKind::RunDone => "A tool run finished successfully.",
            EventKind::RunFailed => "A tool run ended in an error.",
            EventKind::RunProgress => "A running tool wrote a progress update.",
            EventKind::InboxMessage => "A message arrived in the tenant's inbox.",
            EventKind::TableChanged => "A row was appended to, updated in or deleted from a table.",
            EventKind::StateChanged => "A state key was set or deleted.",
            EventKind::QueueDrained => "A queue went empty after being non-empty.",
            EventKind::QueueDeadLetter => "A queue item exhausted its attempts and moved to dead letters.",
            EventKind::TriggerFired => "A trigger fired; data.trigger_kind says which sort.",
            EventKind::ToolPublished => "A tool was published or republished.",
        }
    }

    /// The `data` fields a delivered event of this kind carries.
    pub fn data_fields(self) -> &'static [&'static str] {
        match self {
            EventKind::RunDone => &["run_id", "tool", "status", "duration_ms"],
            EventKind::RunFailed => &["run_id", "tool", "status", "error_code"],
            EventKind::RunProgress => &["run_id", "tool", "progress"],
            EventKind::InboxMessage => &["message_id", "channel", "from"],
            EventKind::TableChanged => &["table", "op", "row_id"],
            EventKind::StateChanged => &["key", "op"],
            EventKind::QueueDrained => &["queue"],
            EventKind::QueueDeadLetter => &["queue", "item_id", "attempts"],
            EventKind::TriggerFired => &["trigger_id", "trigger_kind"],
            EventKind::ToolPublished => &["tool", "version"],
        }
    }

    pub fn parse(s: &str) -> Option<EventKind> {
        Self::ALL.iter().copied().find(|k| k.as_str() == s)
    }
}

/// Every queue state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueState {
    Active,
    Paused,
    Draining,
    Drained,
}

impl QueueState {
    pub const ALL: &'static [QueueState] =
        &[QueueState::Active, QueueState::Paused, QueueState::Draining, QueueState::Drained];

    pub fn as_str(self) -> &'static str {
        match self {
            QueueState::Active => "active",
            QueueState::Paused => "paused",
            QueueState::Draining => "draining",
            QueueState::Drained => "drained",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            QueueState::Active => "Accepting and delivering items.",
            QueueState::Paused => "Accepting items; delivery is held.",
            QueueState::Draining => "Refusing new items; delivering what remains.",
            QueueState::Drained => "Empty and refusing new items.",
        }
    }
}

/// The JSON-schema fragment every tool argument naming a kind embeds.
pub fn schema_enum() -> Value {
    json!({"type": "string", "enum": EventKind::ALL.iter().map(|k| k.as_str()).collect::<Vec<_>>()})
}

/// Where the generated copies live, relative to the repo root.
pub const EVENTS_DOC_PATH: &str = "docs/EVENTS.md";
pub const EVENTS_JSON_PATH: &str = "www/event-kinds.json";
/// The synthorg harness copy, written by hand once at landing (the host repo
/// cannot write across repos); `www/event-kinds.json` is what it diffs against.
pub const HARNESS_CORPUS_PATH: &str = "corpora/mcphost/event-kinds.json";

/// `GET /events.json` body.
pub fn events_json() -> Value {
    json!({
        "kinds": EventKind::ALL.iter().map(|k| json!({
            "kind": k.as_str(),
            "description": k.description(),
            "data_fields": k.data_fields(),
        })).collect::<Vec<_>>(),
        "queue_states": QueueState::ALL.iter().map(|s| json!({
            "state": s.as_str(),
            "description": s.description(),
        })).collect::<Vec<_>>(),
        "generated_from": "src/events.rs",
    })
}

/// `www/event-kinds.json` (the harness file): `{kinds: [..], queue_states: [..]}`.
pub fn render_event_kinds_json() -> String {
    let v = json!({
        "kinds": EventKind::ALL.iter().map(|k| k.as_str()).collect::<Vec<_>>(),
        "queue_states": QueueState::ALL.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
    });
    format!("{}\n", serde_json::to_string_pretty(&v).expect("static json serializes"))
}

/// The whole `docs/EVENTS.md`.
pub fn render_events_doc() -> String {
    let mut out = String::from("# Events\n\n");
    out.push_str(
        "GENERATED by `mcphost gen-docs` from `src/events.rs` -- do not edit by hand. \
         Machine-readable: [`/events.json`](/events.json).\n\n",
    );
    out.push_str("## Generated files\n\n");
    out.push_str(&format!(
        "- `{EVENTS_DOC_PATH}` (this file)\n- `{EVENTS_JSON_PATH}` (harness copy; synthorg keeps its own at `{HARNESS_CORPUS_PATH}`)\n- `www/llms.txt` (`## Events` section)\n\n"
    ));
    out.push_str("## Event kinds\n\n| kind | description | data fields |\n|---|---|---|\n");
    for k in EventKind::ALL {
        let fields = k.data_fields().iter().map(|f| format!("`{f}`")).collect::<Vec<_>>().join(", ");
        out.push_str(&format!("| `{}` | {} | {fields} |\n", k.as_str(), k.description()));
    }
    out.push_str("\n## Queue states\n\n| state | description |\n|---|---|\n");
    for s in QueueState::ALL {
        out.push_str(&format!("| `{}` | {} |\n", s.as_str(), s.description()));
    }
    out
}

/// Curated near-misses an edit distance cannot reach (`finished` is six edits
/// from `done`); checked before the distance match.
const KIND_HINTS: &[(&str, EventKind)] = &[
    ("run.finished", EventKind::RunDone),
    ("run.complete", EventKind::RunDone),
    ("run.completed", EventKind::RunDone),
    ("run.success", EventKind::RunDone),
    ("run.error", EventKind::RunFailed),
    ("run.failure", EventKind::RunFailed),
];

/// Largest edit distance still offered as `did_you_mean`.
const MAX_DID_YOU_MEAN_DISTANCE: usize = 2;

fn did_you_mean(s: &str) -> Option<&'static str> {
    let lower = s.to_ascii_lowercase();
    if let Some((_, k)) = KIND_HINTS.iter().find(|(h, _)| *h == lower) {
        return Some(k.as_str());
    }
    EventKind::ALL
        .iter()
        .map(|k| (crate::errors::levenshtein(&lower, k.as_str()), k.as_str()))
        .min_by_key(|(d, _)| *d)
        .filter(|(d, _)| *d <= MAX_DID_YOU_MEAN_DISTANCE)
        .map(|(_, k)| k)
}

/// Render a bad `kind` argument through `ArgsError` (argmsg_ac06: one message
/// shape, one `data` shape) plus the R7 `expected_one_of` / `did_you_mean`.
fn kind_args_invalid(got: String, raw: String, suggestion: Option<&str>) -> AppError {
    let expected: Vec<&str> = EventKind::ALL.iter().map(|k| k.as_str()).collect();
    let args_err = crate::kinds::ArgsError {
        phase: "args_coercion",
        instance_path: "/kind".to_string(),
        argument: Some("kind".to_string()),
        field: Some("kind".to_string()),
        expected_type: None,
        actual_type: None,
        got,
        docs: "host.events",
        raw,
    };
    let mut data = args_err.data();
    data["expected_one_of"] = json!(expected);
    data["did_you_mean"] = json!(suggestion);
    AppError::Structured {
        code: crate::errors::ARGS_INVALID,
        message: args_err.message(),
        data,
    }
}

/// R7: parse a `kind` argument. An unknown spelling is `args_invalid` with
/// `data.expected_one_of` (= [`EventKind::ALL`]) and `data.did_you_mean`.
pub fn parse_kind(s: &str) -> Result<EventKind, AppError> {
    if let Some(k) = EventKind::parse(s) {
        return Ok(k);
    }
    let expected: Vec<&str> = EventKind::ALL.iter().map(|k| k.as_str()).collect();
    Err(kind_args_invalid(
        format!("{s:?}"),
        format!("unknown event kind '{s}'; expected one of: {}", expected.join(", ")),
        did_you_mean(s),
    ))
}

/// Read and validate the `kind` argument of a tool that embeds [`schema_enum`].
pub fn kind_arg(args: &Value) -> Result<EventKind, AppError> {
    match args.get("kind").and_then(Value::as_str) {
        Some(s) => parse_kind(s),
        None => Err(kind_args_invalid("null".to_string(), "kind is required".to_string(), None)),
    }
}
