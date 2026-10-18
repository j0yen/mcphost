//! PRD-mcphost-runs-list-filter-enums-generated R1: the one Rust source of
//! the `runs.status` and `runs.trigger` value sets. The `host.runs.list` /
//! `admin.runs` descriptors (`schema_property`), their description text
//! (`names().join("|")`), the handler parse and every store write
//! (`as_str`) are all derived from these lists, so the schema, the runtime
//! set and the rows the store holds cannot drift (the drift test enumerates
//! the write sites).

use serde_json::{Value, json};

macro_rules! run_enum {
    ($(#[$meta:meta])* $name:ident, $what:literal, [$($variant:ident => $text:literal),+ $(,)?]) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            pub fn as_str(self) -> &'static str {
                match self {
                    $($name::$variant => $text),+
                }
            }

            pub fn parse(raw: &str) -> Option<$name> {
                Self::ALL.iter().copied().find(|v| v.as_str() == raw)
            }

            pub fn names() -> Vec<&'static str> {
                Self::ALL.iter().map(|v| v.as_str()).collect()
            }

            /// The input-schema property for a filter on this set.
            pub fn schema_property() -> Value {
                json!({
                    "type": "string",
                    "enum": Self::names(),
                    "description": format!(
                        concat!("Only runs ", $what, " (", "{}", ")."),
                        Self::names().join("|")
                    ),
                })
            }
        }
    };
}

run_enum!(
    /// Every value the store writes into `runs.status`.
    RunStatus,
    "in this status",
    [
        Queued => "queued",
        Running => "running",
        Done => "done",
        Error => "error",
        Timeout => "timeout",
        Cancelled => "cancelled",
        Rejected => "rejected",
        Skipped => "skipped",
        // A composed step run (`trigger: composition`) is written
        // `in_flight` when the step starts and settles to `done`/`failed`.
        InFlight => "in_flight",
        Failed => "failed",
    ]
);

run_enum!(
    /// Every value the store writes into `runs.trigger`.
    RunTrigger,
    "of this trigger kind",
    [
        Call => "call",
        Job => "job",
        Schedule => "schedule",
        Event => "event",
        Message => "message",
        Webhook => "webhook",
        Composition => "composition",
        Url => "url",
    ]
);

impl RunStatus {
    /// A run that will not change again (`host.runs.wait` returns on it).
    /// `rejected` and `skipped` are written with `finished_unix` set.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            RunStatus::Done
                | RunStatus::Error
                | RunStatus::Timeout
                | RunStatus::Cancelled
                | RunStatus::Rejected
                | RunStatus::Skipped
                | RunStatus::Failed
        )
    }

    /// [`Self::is_terminal`] over a stored status string.
    pub fn is_terminal_str(raw: &str) -> bool {
        Self::parse(raw).is_some_and(Self::is_terminal)
    }
}

impl RunTrigger {
    /// The kinds `host.trigger.set` accepts (`src/triggers.rs`): a stored
    /// trigger kind, as opposed to how a single run was started.
    pub const SETTABLE: &'static [RunTrigger] =
        &[RunTrigger::Schedule, RunTrigger::Event, RunTrigger::Message, RunTrigger::Webhook];
}
