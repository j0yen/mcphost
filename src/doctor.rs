//! PRD-mcphost-host-doctor-tool: `host.doctor` groups a tenant's recent
//! failed calls by family and says what to do next.
//!
//! `calls.error_summary` (written by [`error_summary`]) is the only input:
//! `"<code>: <message first line>"`, then `\n<source line>` for a python
//! `tool_exception`. [`parse_summary`] reads those fields back, [`family_of`]
//! names the family, and [`DOCTOR_NEXT`] holds one next-step template per
//! family. [`DOCTOR_FIXTURES`] pairs every family with a summary it renders
//! against, and a test asserts the two tables cannot drift.

use serde_json::{Value, json};

/// The most bytes `calls.error_summary` may hold.
pub const MAX_SUMMARY_BYTES: usize = 512;
/// The most characters a rendered `next` may hold.
pub const MAX_NEXT_CHARS: usize = 240;
/// The most characters one substituted value may add to a `next`.
const MAX_VALUE_CHARS: usize = 80;

/// One next-step template per error family. Placeholders: `{tool}`,
/// `{line}`, `{argument}`, `{expected_type}`, `{ceiling}`, `{upstream_status}`.
pub const DOCTOR_NEXT: &[(&str, &str)] = &[
    (
        "budget_ceiling",
        "`{tool}` asked for {argument} above this plan's ceiling of {ceiling}: send {ceiling} or less, \
         or upgrade the plan (host.quickstart limits.plan lists every ceiling)",
    ),
    (
        "tool_exception",
        "`{tool}` raised at tool.py:{line}: patch that line (first_source_line shows it), then \
         host.tool_test it; host.call_trace on a failed call_id has the traceback",
    ),
    (
        "tool_name_error",
        "`{tool}` uses a name that is not defined at tool.py:{line}: define or import it there; \
         host.quickstart lists the modules the sandbox provides",
    ),
    (
        "args_invalid",
        "`{tool}` refused {argument}: send it as {expected_type}; nothing is coerced, so fix the \
         call, or the tool's args_schema if the schema is wrong",
    ),
    (
        "upstream_status",
        "`{tool}`'s upstream answered HTTP {upstream_status}: check the url, credential and \
         method it sends; host.tool_logs name={tool} shows the exchange",
    ),
    (
        "call_timeout",
        "`{tool}` ran past its deadline: raise its timeout_s within the plan, or make it do less \
         per call; host.tool_logs name={tool} shows where it stopped",
    ),
    (
        "other",
        "`{tool}` failed; host.tool_logs name={tool} has its recent log lines and host.call_trace \
         the full failed call",
    ),
];

/// `(family, tool, summary)`: one realistic summary per [`DOCTOR_NEXT`] row.
pub const DOCTOR_FIXTURES: &[(&str, &str, &str)] = &[
    (
        "budget_ceiling",
        "summarize",
        "budget_ceiling_exceeded: budget.max_child_calls: 99999 exceeds this plan's ceiling of 30000",
    ),
    (
        "tool_exception",
        "lookup",
        "tool_exception: TypeError: string indices must be integers, not 'str' at tool.py:16\nname = rows[\"name\"]",
    ),
    (
        "tool_name_error",
        "report",
        "tool_exception: NameError: name 'mcphost' is not defined at tool.py:7\nrows = mcphost.state.query(table=T)",
    ),
    ("args_invalid", "count_rows", "args_invalid: args.limit: expected integer, got number"),
    ("upstream_status", "fetch_issue", "upstream_status: upstream returned HTTP 403 (upstream_status)"),
    ("call_timeout", "slow_report", "call_timeout: the call ran past its 30 s deadline"),
    ("other", "mystery", "storage: the database is busy"),
];

/// `calls.error_summary` for a failed call, from the error's `code`,
/// `message` and `data.line` / `data.source_line` only -- never an argument
/// value (an `args_invalid` message drops its `(<received excerpt>)`) and
/// never a credential (key=value pairs and bearer tokens are masked).
pub fn error_summary(code: &str, message: &str, data: &Value) -> String {
    let first = message.lines().next().unwrap_or("").trim();
    let first = if code == "args_invalid" {
        first.split_once(" (").map_or(first, |(head, _)| head)
    } else {
        first
    };
    let mut out = format!("{code}: {}", redact(first));
    if let Some(n) = data.get("line").and_then(Value::as_i64)
        && !out.contains(" at tool.py:")
    {
        out.push_str(&format!(" at tool.py:{n}"));
    }
    if let Some(src) = data.get("source_line").and_then(Value::as_str) {
        let src = src.lines().next().unwrap_or("").trim();
        if !src.is_empty() {
            out.push('\n');
            out.push_str(&redact(src));
        }
    }
    truncate_bytes(out, MAX_SUMMARY_BYTES)
}

/// [`error_summary`] for an [`AppError`]: its code, display message and, for
/// a structured error, its `data`.
pub fn error_summary_of(err: &crate::errors::AppError) -> String {
    let null = Value::Null;
    let data = match err {
        crate::errors::AppError::Structured { data, .. } => data,
        _ => &null,
    };
    error_summary(err.code(), &err.to_string(), data)
}

/// Masks `name=value` tokens whose name looks like a credential and any
/// `Bearer <token>`.
fn redact(text: &str) -> String {
    let mut out = Vec::new();
    let mut after_bearer = false;
    for word in text.split(' ') {
        if after_bearer {
            out.push("***".to_string());
            after_bearer = false;
        } else if word.eq_ignore_ascii_case("bearer") {
            out.push(word.to_string());
            after_bearer = true;
        } else if let Some((name, _)) = word.split_once('=')
            && ["key", "secret", "token", "password", "auth"]
                .iter()
                .any(|k| name.to_ascii_lowercase().contains(k))
        {
            out.push(format!("{name}=***"));
        } else {
            out.push(word.to_string());
        }
    }
    out.join(" ")
}

fn truncate_bytes(mut s: String, max: usize) -> String {
    if s.len() > max {
        let mut cut = max;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
    }
    s
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max.saturating_sub(1)).chain(std::iter::once('…')).collect()
    }
}

/// The fields [`error_summary`] wrote, read back.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Parsed {
    pub code: String,
    /// The exception class of a `tool_exception` (`TypeError`).
    pub class: Option<String>,
    pub line: Option<i64>,
    pub source_line: Option<String>,
    pub argument: Option<String>,
    pub expected_type: Option<String>,
    pub ceiling: Option<String>,
    pub upstream_status: Option<String>,
}

fn number_after(text: &str, marker: &str) -> Option<String> {
    let rest = &text[text.find(marker)? + marker.len()..];
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    (!digits.is_empty()).then_some(digits)
}

/// Reads a stored summary; `error_class` covers a row with no summary (a
/// call recorded before migration 0097).
pub fn parse_summary(summary: Option<&str>, error_class: &str) -> Parsed {
    let Some(summary) = summary else {
        return Parsed { code: error_class.to_string(), ..Parsed::default() };
    };
    let (head, source) = match summary.split_once('\n') {
        Some((h, s)) => (h, Some(s.trim().to_string()).filter(|s| !s.is_empty())),
        None => (summary, None),
    };
    let (code, message) = head.split_once(": ").unwrap_or((error_class, head));
    let mut parsed = Parsed {
        code: code.to_string(),
        source_line: source,
        line: number_after(message, " at tool.py:").and_then(|n| n.parse().ok()),
        ceiling: number_after(message, "ceiling of "),
        upstream_status: number_after(message, "HTTP "),
        ..Parsed::default()
    };
    if code == "tool_exception"
        && let Some((class, _)) = message.split_once(": ")
        && !class.is_empty()
        && class.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
    {
        parsed.class = Some(class.to_string());
    }
    if let Some((field, rest)) = message.split_once(": ")
        && (field.starts_with("args.") || field.starts_with("budget."))
    {
        parsed.argument = Some(field.strip_prefix("args.").unwrap_or(field).to_string());
        if let Some(expected) = rest.strip_prefix("expected ") {
            parsed.expected_type = expected.split(',').next().map(|t| t.trim().to_string());
        }
    }
    parsed
}

/// The [`DOCTOR_NEXT`] family a failure belongs to.
pub fn family_of(parsed: &Parsed) -> &'static str {
    match parsed.code.as_str() {
        "budget_ceiling_exceeded" => "budget_ceiling",
        "tool_exception" => match parsed.class.as_deref() {
            Some("NameError" | "UnboundLocalError") => "tool_name_error",
            _ => "tool_exception",
        },
        "args_invalid" => "args_invalid",
        "call_timeout" => "call_timeout",
        code if code.starts_with("upstream") => "upstream_status",
        _ => "other",
    }
}

/// Renders a family's template for `tool` from `parsed`. Every placeholder
/// is filled -- a field the summary does not carry reads `unknown` -- and
/// the result is at most [`MAX_NEXT_CHARS`] characters.
pub fn render_next(family: &str, tool: &str, parsed: &Parsed) -> String {
    let template = DOCTOR_NEXT
        .iter()
        .find(|(f, _)| *f == family)
        .or_else(|| DOCTOR_NEXT.iter().find(|(f, _)| *f == "other"))
        .map_or("", |(_, t)| *t);
    let unknown = || "unknown".to_string();
    let values: [(&str, String); 6] = [
        ("{tool}", tool.to_string()),
        ("{line}", parsed.line.map_or_else(unknown, |n| n.to_string())),
        ("{argument}", parsed.argument.clone().unwrap_or_else(unknown)),
        ("{expected_type}", parsed.expected_type.clone().unwrap_or_else(unknown)),
        ("{ceiling}", parsed.ceiling.clone().unwrap_or_else(unknown)),
        ("{upstream_status}", parsed.upstream_status.clone().unwrap_or_else(unknown)),
    ];
    let mut out = template.to_string();
    for (placeholder, value) in values {
        out = out.replace(placeholder, &truncate_chars(&value, MAX_VALUE_CHARS));
    }
    truncate_chars(&out, MAX_NEXT_CHARS)
}

/// One failed `calls` row, as `host.doctor` reads it.
#[derive(Debug, Clone)]
pub struct FailedCall {
    pub tool: String,
    pub error_class: String,
    pub error_summary: Option<String>,
    /// `calls.outcome`: `error`, `timeout`, or `refused` (nothing ran).
    pub outcome: String,
    pub started_at: String,
}

/// Groups `rows` (newest first) by `(error_class, exception class)`, biggest
/// group first, ties newest first.
pub fn build_groups(rows: &[FailedCall]) -> Vec<Value> {
    struct Group<'a> {
        code: String,
        class: Option<String>,
        count: usize,
        outcomes: std::collections::BTreeMap<String, usize>,
        tools: Vec<&'a str>,
        newest: &'a FailedCall,
        newest_parsed: Parsed,
        source: Option<(String, Option<i64>)>,
    }
    let mut groups: Vec<Group> = Vec::new();
    for row in rows {
        let parsed = parse_summary(row.error_summary.as_deref(), &row.error_class);
        let code = row.error_class.clone();
        let idx = match groups.iter().position(|g| g.code == code && g.class == parsed.class) {
            Some(i) => i,
            None => {
                groups.push(Group {
                    code,
                    class: parsed.class.clone(),
                    count: 0,
                    outcomes: std::collections::BTreeMap::new(),
                    tools: Vec::new(),
                    newest: row,
                    newest_parsed: parsed.clone(),
                    source: None,
                });
                groups.len() - 1
            }
        };
        let g = &mut groups[idx];
        g.count += 1;
        *g.outcomes.entry(row.outcome.clone()).or_default() += 1;
        if !g.tools.contains(&row.tool.as_str()) {
            g.tools.push(&row.tool);
        }
        if g.source.is_none()
            && let Some(src) = &parsed.source_line
        {
            g.source = Some((src.clone(), parsed.line));
        }
    }
    // `rows` is newest first, so groups were pushed in order of their newest
    // row; the stable sort keeps that order among equal counts (count, then
    // newest `last_ts`).
    groups.sort_by_key(|g| std::cmp::Reverse(g.count));
    groups
        .into_iter()
        .map(|g| {
            let family = family_of(&g.newest_parsed);
            let mut parsed = g.newest_parsed.clone();
            if let Some((_, line)) = &g.source {
                parsed.line = parsed.line.or(*line);
            }
            let mut out = json!({
                "code": g.code,
                "family": family,
                "class": g.class,
                "count": g.count,
                "outcomes": g.outcomes,
                "tools": g.tools,
                "last_ts": g.newest.started_at,
                "next": render_next(family, &g.newest.tool, &parsed),
            });
            if g.code == "tool_exception" {
                if let Some((src, line)) = &g.source {
                    out["first_source_line"] = json!(src);
                    if let Some(n) = line {
                        out["line"] = json!(n);
                    }
                } else if let Some(n) = parsed.line {
                    out["line"] = json!(n);
                }
            }
            out
        })
        .collect()
}

/// The `next` of an empty window.
pub fn empty_next(limit: i64) -> String {
    format!("no failed calls in the last {limit}; publish and call a tool, then ask again")
}
