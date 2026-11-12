//! PRD-mcphost-pii-tagging-and-log-redaction-v2: the registry of every
//! persisted sink of tool output, and the one redaction every write into a
//! sink passes through.
//!
//! A tool's publisher tags the arguments that carry personal data
//! (`pii: ["email"]` on `host.tool.publish`, or `"x-pii": true` on a schema
//! property). The call's tagged values become a [`PiiSet`] held in a per-call
//! task-local, the way `subkeys::CALL_KEY` carries the key id, so no writer
//! signature changes. Each writer named in [`LogSink::writers`] calls
//! [`LogSink::write_through`] on the text it persists; the drift test
//! `piir_sinks_cover_every_write` scans the source for a write that does not.

use std::cell::RefCell;
use std::fmt;
use std::future::Future;
use std::sync::Arc;

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::Value;

/// What replaces a tagged value in a sink.
pub const REDACTION: &str = "[redacted:pii]";

/// Tagged values shorter than this stay readable (`NY`): the floor is 3, one
/// below the secret redactor's `MIN_NEEDLE`, on purpose.
pub const MIN_PII_NEEDLE: usize = 3;

/// The persisted sinks of tool output. The `Display` names are the sink list
/// a contract or a posture reports; nothing else lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LogSink {
    /// `logs.line`, what a kind writes through `CallLog` (`host.tool.logs`).
    ToolLog,
    /// `call_traces`: the request, both output tails and the error.
    CallTrace,
    /// `calls.error_summary`.
    CallErrorSummary,
    /// `runs.error_data_json`, an async run's error data.
    RunError,
    /// `messages`, a message written while a tool call runs.
    Message,
}

impl fmt::Display for LogSink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            LogSink::ToolLog => "ToolLog",
            LogSink::CallTrace => "CallTrace",
            LogSink::CallErrorSummary => "CallErrorSummary",
            LogSink::RunError => "RunError",
            LogSink::Message => "Message",
        })
    }
}

impl LogSink {
    /// Every variant. `piir_ac07` reads the enum's own source and fails when a
    /// variant is missing here.
    pub const ALL: [LogSink; 5] =
        [LogSink::ToolLog, LogSink::CallTrace, LogSink::CallErrorSummary, LogSink::RunError, LogSink::Message];

    /// The `Display` name of every variant, in declaration order: the list the
    /// generated contract carries.
    pub fn names() -> Vec<String> {
        Self::ALL.iter().map(ToString::to_string).collect()
    }

    /// The functions that write into this sink, as `(file, function)`. The
    /// drift test requires every write into a sink table or column to sit in
    /// one of these or in its allowlist.
    pub fn writers(self) -> &'static [(&'static str, &'static str)] {
        match self {
            LogSink::ToolLog => &[("src/db.rs", "append_log")],
            LogSink::CallTrace => &[("src/db.rs", "insert_call_trace")],
            LogSink::CallErrorSummary => {
                &[("src/db.rs", "record_call_traced"), ("src/db.rs", "record_refused_call")]
            }
            LogSink::RunError => &[("src/db.rs", "finalize_run")],
            LogSink::Message => &[("src/db.rs", "insert_message")],
        }
    }

    /// `text` with the current call's tagged values replaced by
    /// [`REDACTION`]; outside a tool call, or for an untagged tool, `text`
    /// unchanged.
    pub fn write_through(self, text: &str) -> String {
        write_through(self, text)
    }

    /// [`Self::write_through`] with an explicit `pii` set, for a writer whose
    /// insert runs on `with_conn`'s blocking thread, where the task-local is
    /// not visible: it reads [`current`] first and passes the set in.
    pub fn write_through_with(self, pii: &PiiSet, text: &str) -> String {
        let _ = self;
        pii.redact(text)
    }
}

/// A function that writes into a sink table or column without going through
/// [`LogSink::write_through`], and why that is safe. `piir_sinks_cover_every_write`
/// fails on a write that is in neither [`LogSink::writers`] nor this list.
#[derive(Clone, Copy, Debug)]
pub struct AllowedWriter {
    pub file: &'static str,
    pub function: &'static str,
    pub reason: &'static str,
}

/// Writers outside the sink table, each with its reason.
///
/// A message a person wrote by hand through `host.msg.send` is not here: it
/// goes through [`LogSink::Message`]'s `insert_message`, and a call that is
/// not a tool call carries no tagged values, so it is stored as written.
pub const ALLOWLIST: &[AllowedWriter] = &[
    AllowedWriter {
        file: "src/db.rs",
        function: "insert_contact_request_notice",
        reason: "host-authored notice of a contact request: fixed text and the requester's namespace, no tool output",
    },
    AllowedWriter {
        file: "src/db.rs",
        function: "insert_alert_notice",
        reason: "host-authored operator alert: the alert's severity, title and id, no tool output",
    },
    AllowedWriter {
        file: "src/db.rs",
        function: "insert_host_notice",
        reason: "host-authored notice from the host's own code paths, no tool output",
    },
    AllowedWriter {
        file: "src/db.rs",
        function: "insert_email_notice",
        reason: "host-authored notice that an email was received or refused: addresses and a fixed sentence, no tool output",
    },
    AllowedWriter {
        file: "src/tasks.rs",
        function: "insert_dead_message",
        reason: "host-authored `task.dead` message: the task id and the fixed reason `deadline`, no tool output",
    },
];

/// [`LogSink::write_through`]: the one redaction every sink write passes.
pub fn write_through(sink: LogSink, text: &str) -> String {
    sink.write_through_with(&current(), text)
}

/// Replaces the current call's tagged values in `text`.
pub fn redact_pii(text: &str) -> String {
    current().redact(text)
}

/// A call's tagged argument names and the needles their values expand to.
#[derive(Clone, Debug, Default)]
pub struct PiiSet {
    keys: Vec<String>,
    needles: Vec<(String, String)>,
}

/// Everything but RFC 3986 unreserved characters: the form a URL component
/// carries (`a%40b.io`); [`NON_ALPHANUMERIC`] covers the stricter one (`a%40b%2Eio`).
const COMPONENT: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'~');

impl PiiSet {
    /// The tagged `names` present in `args`: each name's string and number
    /// leaves (objects and arrays flattened) become needles of at least
    /// [`MIN_PII_NEEDLE`] characters, in raw, JSON-escaped and percent-encoded
    /// form.
    pub fn from_args(names: &[String], args: &Value) -> Self {
        let mut leaves = Vec::new();
        for name in names {
            if let Some(v) = args.get(name) {
                flatten(v, &mut leaves);
            }
        }
        let mut forms: Vec<String> = Vec::new();
        for leaf in leaves.into_iter().filter(|l| l.chars().count() >= MIN_PII_NEEDLE) {
            let json_escaped = serde_json::to_string(&leaf).map(|j| j[1..j.len() - 1].to_string()).unwrap_or_default();
            let encoded_component = utf8_percent_encode(&leaf, COMPONENT).to_string();
            let encoded_strict = utf8_percent_encode(&leaf, NON_ALPHANUMERIC).to_string();
            let form_encoded = encoded_component.replace("%20", "+");
            for form in [leaf, json_escaped, encoded_component, encoded_strict, form_encoded] {
                if !form.is_empty() && !forms.contains(&form) {
                    forms.push(form);
                }
            }
        }
        forms.sort_by_key(|f| std::cmp::Reverse(f.len()));
        Self {
            keys: names.to_vec(),
            needles: forms.into_iter().map(|f| (f, REDACTION.to_string())).collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// The tagged argument names.
    pub fn keys(&self) -> &[String] {
        &self.keys
    }

    /// `(value form, replacement)`, longest first.
    pub fn needles(&self) -> &[(String, String)] {
        &self.needles
    }

    pub fn redact(&self, text: &str) -> String {
        crate::calltrace::scrub(&self.needles, text)
    }
}

fn flatten(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Number(n) => out.push(n.to_string()),
        Value::Array(items) => items.iter().for_each(|i| flatten(i, out)),
        Value::Object(map) => map.values().for_each(|i| flatten(i, out)),
        Value::Bool(_) | Value::Null => {}
    }
}

tokio::task_local! {
    /// The tagged values of the tool call running on this task. Filled by
    /// [`arm`] once the tool row and the arguments are known, read by every
    /// sink writer.
    static ARMED: RefCell<Arc<PiiSet>>;
}

/// Runs `fut` with an empty armed set: a sink write inside it sees whatever
/// [`arm`] stored, a write outside any scope sees none.
pub async fn scoped<F: Future>(fut: F) -> F::Output {
    ARMED.scope(RefCell::new(Arc::new(PiiSet::default())), fut).await
}

/// Arms the current scope with the tagged values `args` holds under `names`;
/// a no-op for an untagged tool or outside a [`scoped`] call.
pub fn arm(names: &[String], args: &Value) {
    if names.is_empty() {
        return;
    }
    let set = Arc::new(PiiSet::from_args(names, args));
    let _ = ARMED.try_with(|cell| *cell.borrow_mut() = set);
}

/// The current call's tagged values (empty outside a tool call).
pub fn current() -> Arc<PiiSet> {
    ARMED.try_with(|cell| cell.borrow().clone()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn expands_leaves_and_encodings_longest_first() {
        let set = PiiSet::from_args(&["email".to_string()], &json!({"email": "a@b.io", "other": "keep@me.io"}));
        let forms: Vec<&str> = set.needles().iter().map(|(f, _)| f.as_str()).collect();
        assert!(forms.contains(&"a@b.io") && forms.contains(&"a%40b.io") && forms.contains(&"a%40b%2Eio"), "{forms:?}");
        assert!(!forms.iter().any(|f| f.contains("keep")));
        assert!(forms.windows(2).all(|w| w[0].len() >= w[1].len()));
    }

    #[test]
    fn redaction_is_stable_under_a_second_pass() {
        let set = PiiSet::from_args(&["v".to_string()], &json!({"v": "pii"}));
        let once = set.redact("a pii b");
        assert_eq!(once, "a [redacted:pii] b");
        assert_eq!(set.redact(&once), once);
    }

    #[tokio::test]
    async fn write_through_redacts_only_inside_an_armed_scope() {
        assert_eq!(LogSink::ToolLog.write_through("x a@b.io"), "x a@b.io");
        scoped(async {
            assert_eq!(LogSink::ToolLog.write_through("x a@b.io"), "x a@b.io");
            arm(&["email".to_string()], &json!({"email": "a@b.io"}));
            assert_eq!(LogSink::ToolLog.write_through("x a@b.io"), "x [redacted:pii]");
        })
        .await;
        assert_eq!(LogSink::ToolLog.write_through("x a@b.io"), "x a@b.io");
    }
}
