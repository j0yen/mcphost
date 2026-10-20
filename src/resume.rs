//! PRD-mcphost-resume-tool: `host.resume {}` -- one call that answers "what
//! was I doing?" for a returning agent: the tenant's tools with each one's
//! last call, unfinished runs, the newest notes, what changed on the host
//! since the agent's last call, and one suggested next call.
//!
//! The response is bounded by construction, not by prose: every cap lives in
//! [`ResumeBudget`] (no cap is a literal anywhere else), [`render`] cuts each
//! section to its cap and then sheds items in a fixed order until the
//! serialized value is at most [`ResumeBudget::max_bytes`], and
//! `truncated.<section>` names every section that lost something. The next
//! call comes from the one table [`RESUME_NEXT`], keyed by a state derived
//! from the tenant's tools and runs.

use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Map, Value, json};

use crate::db::Tenant;
use crate::errors::AppError;
use crate::notes::Attach;
use crate::run_enums::RunStatus;
use crate::state::{AppState, rfc3339_from_unix};

pub const TOOL_NAME: &str = "host.resume";

/// Tools an anonymous caller on bare `/mcp` may call without being implicitly
/// signed up. The one list the starter `tools/list` must contain
/// (`handler::STARTER_TOOL_NAMES` is checked against it) and the
/// implicit-signup gate skips; neither keeps a literal copy.
pub const ANON_SAFE_TOOLS: &[&str] = &[
    "host.quickstart",
    "host.help.search",
    "host.help.get",
    "host.errors",
    "host.redeem",
    "billing.plans",
    TOOL_NAME,
];

/// Every cap on a `host.resume` response, in one place.
#[derive(Debug, Clone, Copy)]
pub struct ResumeBudget {
    /// The whole serialized response.
    pub max_bytes: usize,
    /// Tool entries shown (most recently called first, then never-called by name).
    pub tools: usize,
    /// Notes shown, newest first.
    pub notes: usize,
    /// Characters of one note's text.
    pub note_chars: usize,
    /// Open runs shown, newest first.
    pub runs: usize,
    /// Releases shown in `host.changes`.
    pub changes: usize,
    /// Characters of one release summary once the response has to shrink.
    pub change_chars: usize,
}

pub const BUDGET: ResumeBudget = ResumeBudget {
    max_bytes: 4096,
    tools: 25,
    notes: 10,
    note_chars: 200,
    runs: 10,
    changes: 5,
    change_chars: 100,
};

/// The state `next_call` is derived from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeState {
    NoTools,
    ToolUntested,
    LastCallFailed,
    RunOpen,
    AllGreen,
}

impl ResumeState {
    pub const COUNT: usize = 5;

    /// The state's row in [`RESUME_NEXT`]. An exhaustive `match`: a new
    /// variant does not compile until it names a slot.
    pub const fn slot(self) -> usize {
        match self {
            ResumeState::NoTools => 0,
            ResumeState::ToolUntested => 1,
            ResumeState::LastCallFailed => 2,
            ResumeState::RunOpen => 3,
            ResumeState::AllGreen => 4,
        }
    }

    pub const ALL: [ResumeState; Self::COUNT] = [
        ResumeState::NoTools,
        ResumeState::ToolUntested,
        ResumeState::LastCallFailed,
        ResumeState::RunOpen,
        ResumeState::AllGreen,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ResumeState::NoTools => "no_tools",
            ResumeState::ToolUntested => "tool_untested",
            ResumeState::LastCallFailed => "last_call_failed",
            ResumeState::RunOpen => "run_open",
            ResumeState::AllGreen => "all_green",
        }
    }
}

/// `(state, tool, why)`: the one suggested next call per [`ResumeState`].
/// Every tool named here must be in `host_tools` (drift test).
pub const RESUME_NEXT: &[(ResumeState, &str, &str)] = &[
    (ResumeState::NoTools, "host.quickstart", "no tool published yet: quickstart shows a worked example"),
    (ResumeState::ToolUntested, "host.tool.test", "a published tool has never been called: dry-run it"),
    (ResumeState::LastCallFailed, "host.tool.test", "a tool's last call failed: re-run it under test"),
    (ResumeState::RunOpen, "host.runs.get", "a run has not finished: read its status"),
    (ResumeState::AllGreen, "host.tool.list", "every tool last passed and no run is open: see what you have"),
];

const _: () = {
    assert!(RESUME_NEXT.len() == ResumeState::COUNT);
    let mut i = 0;
    while i < RESUME_NEXT.len() {
        assert!(RESUME_NEXT[i].0.slot() == i);
        i += 1;
    }
};

/// The row `state` names.
pub fn next_row(state: ResumeState) -> &'static (ResumeState, &'static str, &'static str) {
    &RESUME_NEXT[state.slot()]
}

#[derive(Debug, Clone, PartialEq)]
pub struct LastCall {
    pub at: i64,
    pub ok: bool,
    pub error_class: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolEntry {
    pub name: String,
    pub kind: String,
    pub last_call: Option<LastCall>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunEntry {
    pub id: String,
    pub tool: String,
    pub status: String,
    pub started: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NoteEntry {
    pub key: String,
    pub text: String,
    pub attached_to: Option<Attach>,
}

/// Everything [`render`] needs, uncut.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub namespace: String,
    pub last_seen: Option<i64>,
    pub version: String,
    pub version_at_last_call: Option<String>,
    /// `host.changelog`'s `releases`, newest first.
    pub changes: Vec<Value>,
    pub tools: Vec<ToolEntry>,
    pub runs: Vec<RunEntry>,
    pub notes: Vec<NoteEntry>,
}

/// Responses over [`ResumeBudget::max_bytes`] since boot -- `render` sheds
/// until it fits, so this stays 0; it is the counter that would say otherwise.
static OVER_BUDGET: AtomicU64 = AtomicU64::new(0);

pub fn over_budget_total() -> u64 {
    OVER_BUDGET.load(Ordering::Relaxed)
}

/// The state and the (tool-or-run) argument its `next_call` carries, from
/// the uncut snapshot. A failed last call outranks an open run, which
/// outranks a tool never called.
pub fn derive_state(snap: &Snapshot) -> (ResumeState, Value) {
    if snap.tools.is_empty() {
        return (ResumeState::NoTools, json!({}));
    }
    let failed = snap
        .tools
        .iter()
        .filter(|t| t.last_call.as_ref().is_some_and(|c| !c.ok))
        .max_by_key(|t| t.last_call.as_ref().map_or(0, |c| c.at));
    if let Some(t) = failed {
        return (ResumeState::LastCallFailed, json!({"name": t.name}));
    }
    if let Some(r) = snap.runs.first() {
        return (ResumeState::RunOpen, json!({"run_id": r.id}));
    }
    if let Some(t) = snap.tools.iter().find(|t| t.last_call.is_none()) {
        return (ResumeState::ToolUntested, json!({"name": t.name}));
    }
    (ResumeState::AllGreen, json!({}))
}

fn cut_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(max.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

fn note_json(n: &NoteEntry) -> Value {
    let mut obj = Map::new();
    obj.insert("key".to_string(), json!(n.key));
    obj.insert("text".to_string(), json!(n.text));
    match &n.attached_to {
        Some(Attach::Tool(t)) => {
            obj.insert("attached_to".to_string(), json!({"tool": t}));
        }
        Some(Attach::Run(r)) => {
            obj.insert("attached_to".to_string(), json!({"run": r}));
        }
        None => {}
    }
    Value::Object(obj)
}

fn tool_json(t: &ToolEntry) -> Value {
    let last_call = t.last_call.as_ref().map(|c| {
        json!({"at": rfc3339_from_unix(c.at), "ok": c.ok, "error_class": c.error_class})
    });
    json!({"name": t.name, "kind": t.kind, "last_call": last_call})
}

fn run_json(r: &RunEntry) -> Value {
    json!({"id": r.id, "tool": r.tool, "status": r.status, "started": r.started.map(rfc3339_from_unix)})
}

/// One section the response can shed from.
#[derive(Clone, Copy)]
enum Section {
    Tools,
    Notes,
    Runs,
    Changes,
}

/// What [`render`] gives up, in order, when a response is over budget:
/// release summaries shrink first, then items leave from the tail of a
/// section down to its floor.
#[derive(Clone, Copy)]
enum Shed {
    Summaries,
    Pop(Section, usize),
}

const SHED_ORDER: &[Shed] = &[
    Shed::Summaries,
    Shed::Pop(Section::Changes, 2),
    Shed::Pop(Section::Notes, 5),
    Shed::Pop(Section::Changes, 0),
    Shed::Pop(Section::Tools, 12),
    Shed::Pop(Section::Runs, 5),
    Shed::Pop(Section::Notes, 0),
    Shed::Pop(Section::Tools, 0),
    Shed::Pop(Section::Runs, 0),
];

/// The bounded `host.resume` response for `snap`. Pure: the same snapshot
/// always renders the same value, and the value serializes to at most
/// `budget.max_bytes`.
pub fn render(snap: &Snapshot, budget: &ResumeBudget) -> Value {
    let (state, next_args) = derive_state(snap);
    let (_, next_tool, next_why) = *next_row(state);

    // Most recently called first, then never-called by name.
    let mut tools = snap.tools.clone();
    tools.sort_by(|a, b| {
        let (la, lb) = (a.last_call.as_ref().map(|c| c.at), b.last_call.as_ref().map(|c| c.at));
        lb.cmp(&la).then_with(|| a.name.cmp(&b.name))
    });

    let mut cut_tools = tools.len() > budget.tools;
    let mut cut_notes = snap.notes.len() > budget.notes;
    let mut cut_runs = snap.runs.len() > budget.runs;
    let mut cut_changes = snap.changes.len() > budget.changes;

    tools.truncate(budget.tools);
    let mut runs: Vec<RunEntry> = snap.runs.iter().take(budget.runs).cloned().collect();
    let mut changes: Vec<Value> = snap.changes.iter().take(budget.changes).cloned().collect();
    let mut notes: Vec<NoteEntry> = snap
        .notes
        .iter()
        .take(budget.notes)
        .map(|n| {
            if n.text.chars().count() > budget.note_chars {
                cut_notes = true;
            }
            NoteEntry { text: cut_chars(&n.text, budget.note_chars), ..n.clone() }
        })
        .collect();

    let build = |tools: &[ToolEntry],
                 notes: &[NoteEntry],
                 runs: &[RunEntry],
                 changes: &[Value],
                 flags: [bool; 4]| {
        json!({
            "tenant": snap.namespace,
            "last_seen": snap.last_seen.map(rfc3339_from_unix),
            "host": {
                "version": snap.version,
                "version_at_last_call": snap.version_at_last_call,
                "changes": changes,
            },
            "tools": tools.iter().map(tool_json).collect::<Vec<_>>(),
            "runs_open": runs.iter().map(run_json).collect::<Vec<_>>(),
            "notes": notes.iter().map(note_json).collect::<Vec<_>>(),
            "next_call": {"tool": next_tool, "args": next_args, "why": next_why},
            "truncated": {
                "tools": flags[0],
                "notes": flags[1],
                "runs_open": flags[2],
                "changes": flags[3],
            },
        })
    };
    let size = |v: &Value| serde_json::to_vec(v).map_or(usize::MAX, |b| b.len());

    let mut value = build(&tools, &notes, &runs, &changes, [cut_tools, cut_notes, cut_runs, cut_changes]);
    for step in SHED_ORDER {
        if size(&value) <= budget.max_bytes {
            break;
        }
        match *step {
            Shed::Summaries => {
                for c in &mut changes {
                    if let Some(s) = c.get("summary").and_then(Value::as_str)
                        && s.chars().count() > budget.change_chars
                    {
                        c["summary"] = json!(cut_chars(s, budget.change_chars));
                        cut_changes = true;
                    }
                }
            }
            Shed::Pop(section, floor) => loop {
                let len = match section {
                    Section::Tools => tools.len(),
                    Section::Notes => notes.len(),
                    Section::Runs => runs.len(),
                    Section::Changes => changes.len(),
                };
                if len <= floor {
                    break;
                }
                match section {
                    Section::Tools => {
                        tools.pop();
                        cut_tools = true;
                    }
                    Section::Notes => {
                        notes.pop();
                        cut_notes = true;
                    }
                    Section::Runs => {
                        runs.pop();
                        cut_runs = true;
                    }
                    Section::Changes => {
                        changes.pop();
                        cut_changes = true;
                    }
                }
                value = build(&tools, &notes, &runs, &changes, [cut_tools, cut_notes, cut_runs, cut_changes]);
                if size(&value) <= budget.max_bytes {
                    break;
                }
            },
        }
        value = build(&tools, &notes, &runs, &changes, [cut_tools, cut_notes, cut_runs, cut_changes]);
    }
    if size(&value) > budget.max_bytes {
        OVER_BUDGET.fetch_add(1, Ordering::Relaxed);
        debug_assert!(false, "host.resume rendered {} bytes", size(&value));
    }
    value
}

/// The tenant's tools (with each newest `calls` row) and open runs, in one
/// connection: the four indexed reads the PRD names are the tools join, the
/// open-runs read, and the notes and changelog reads done by the caller.
async fn read_tools_and_runs(
    state: &AppState,
    tenant_id: i64,
    run_limit: usize,
) -> Result<(Vec<ToolEntry>, Vec<RunEntry>), AppError> {
    let terminal: Vec<String> = RunStatus::ALL
        .iter()
        .filter(|s| s.is_terminal())
        .map(|s| format!("'{}'", s.as_str()))
        .collect();
    let runs_sql = format!(
        "SELECT id, tool_name, status, started_unix FROM runs \
         WHERE tenant_id = ?1 AND parent_run_id IS NULL AND status NOT IN ({}) \
         ORDER BY rowid DESC LIMIT ?2",
        terminal.join(",")
    );
    state
        .db
        .with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT t.name, t.kind, c.started_unix, c.ok, c.error_class \
                 FROM tools t LEFT JOIN calls c ON c.id = ( \
                     SELECT id FROM calls WHERE tenant_id = t.tenant_id AND tool_name = t.name \
                     ORDER BY started_unix DESC, id DESC LIMIT 1) \
                 WHERE t.tenant_id = ?1",
            )?;
            let tools = stmt
                .query_map(rusqlite::params![tenant_id], |r| {
                    let at: Option<i64> = r.get(2)?;
                    let ok: Option<bool> = r.get(3)?;
                    let error_class: Option<String> = r.get(4)?;
                    Ok(ToolEntry {
                        name: r.get(0)?,
                        kind: r.get(1)?,
                        last_call: at.map(|at| LastCall { at, ok: ok.unwrap_or(false), error_class }),
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            let mut stmt = conn.prepare(&runs_sql)?;
            let runs = stmt
                .query_map(rusqlite::params![tenant_id, run_limit as i64], |r| {
                    Ok(RunEntry { id: r.get(0)?, tool: r.get(1)?, status: r.get(2)?, started: r.get(3)? })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok((tools, runs))
        })
        .await
}

/// A gap longer than this between two of a tenant's calls starts a new session.
pub const SESSION_GAP_S: i64 = 30 * 60;

/// The window `host.usage`'s `resume` block reports over.
pub const USAGE_WINDOW_S: i64 = 7 * 24 * 3600;

/// `(sessions, sessions whose first host.* call was host.resume)` over
/// `(started_unix, tool_name)` rows already in call order. Sessions are
/// delimited by gaps of more than [`SESSION_GAP_S`] (`calls.session_id` once
/// the session journal lands); a session with no host.* call still counts as
/// a session.
pub fn count_first_calls(rows: &[(i64, String)]) -> (u32, u32) {
    let (mut sessions, mut opened_with_resume) = (0, 0);
    let mut previous: Option<i64> = None;
    let mut host_call_seen = false;
    for (at, tool) in rows {
        if previous.is_none_or(|p| at - p > SESSION_GAP_S) {
            sessions += 1;
            host_call_seen = false;
        }
        previous = Some(*at);
        if !host_call_seen && tool.starts_with("host.") {
            host_call_seen = true;
            if tool == TOOL_NAME {
                opened_with_resume += 1;
            }
        }
    }
    (sessions, opened_with_resume)
}

/// R7: the `resume` block of `host.usage` --
/// `first_call_rate_7d` (sessions whose first host.* call was `host.resume`
/// over sessions, `null` with no session), `sessions_7d`, and the host-wide
/// count of responses that came out over budget (always 0 unless `render`
/// is broken).
pub async fn usage_block(state: &AppState, tenant_id: i64, now: i64) -> Result<Value, AppError> {
    let since = now - USAGE_WINDOW_S;
    let rows: Vec<(i64, String)> = state
        .db
        .with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT started_unix, tool_name FROM calls \
                 WHERE tenant_id = ?1 AND started_unix >= ?2 AND caller_tenant_id IS NULL \
                 AND outcome IS NOT 'test' ORDER BY started_unix, id",
            )?;
            let rows = stmt
                .query_map(rusqlite::params![tenant_id, since], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;
    let (sessions, opened_with_resume) = count_first_calls(&rows);
    let rate = (sessions > 0).then(|| (f64::from(opened_with_resume) / f64::from(sessions) * 1000.0).round() / 1000.0);
    Ok(json!({
        "first_call_rate_7d": rate,
        "sessions_7d": sessions,
        "over_budget_total": over_budget_total(),
    }))
}

/// `host.resume {}` for a caller with no tenant: `host.quickstart`'s
/// anonymous block and the call that block starts with. Creates nothing.
pub fn anonymous(onboarding: Value) -> Value {
    let (_, tool, why) = *next_row(ResumeState::NoTools);
    json!({"onboarding": onboarding, "next_call": {"tool": tool, "args": {}, "why": why}})
}

/// `host.resume {}` for an authenticated tenant.
pub async fn resume(state: &AppState, tenant: &Tenant, _args: &Value) -> Result<Value, AppError> {
    let (tools, runs) = read_tools_and_runs(state, tenant.id, BUDGET.runs + 1).await?;

    let now = crate::notes::now(state);
    let notes: Vec<NoteEntry> = crate::notes::live_notes(state, tenant.id, now)
        .await?
        .into_iter()
        .take(BUDGET.notes + 1)
        .map(|n| NoteEntry { key: n.key, text: n.text, attached_to: n.attach_to })
        .collect();

    // `tenant.last_seen_version` was loaded by this call's own auth resolver
    // before it stamped the column with the host's version, so it is the
    // version of the tenant's previous call, and the stamp is the "update
    // after the resume is built".
    let since = tenant.last_seen_version.as_deref().filter(|v| crate::api_contract::Version::parse(v).is_some());
    let log = crate::api_contract::changelog(&state.kinds, &state.deprecations, since);
    let changes: Vec<Value> = log.get("releases").and_then(Value::as_array).cloned().unwrap_or_default();

    let snap = Snapshot {
        namespace: tenant.namespace.clone(),
        last_seen: tenant.last_seen_unix,
        version: env!("CARGO_PKG_VERSION").to_string(),
        version_at_last_call: tenant.last_seen_version.clone(),
        changes,
        tools,
        runs,
        notes,
    };
    let out = render(&snap, &BUDGET);
    // The row `host.usage`'s first-call rate counts: host.* calls leave no
    // `calls` row of their own, so the resume writes the one it is measured by.
    if let Err(e) = state
        .db
        .record_call(
            tenant.id,
            TOOL_NAME.to_string(),
            0,
            true,
            None,
            None,
            None,
            "ok",
            tenant.origin.clone(),
            tenant.origin_detail.clone(),
        )
        .await
    {
        tracing::warn!(tenant = %tenant.namespace, error = %e, "host.resume call row not recorded");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn long(c: char, n: usize) -> String {
        std::iter::repeat_n(c, n).collect()
    }

    /// Every cap populated at once, at the largest sizes the store allows.
    fn worst_case(budget: &ResumeBudget) -> Snapshot {
        Snapshot {
            namespace: long('n', 64),
            last_seen: Some(1_760_000_000),
            version: "0.112.0".to_string(),
            version_at_last_call: Some("0.092.0".to_string()),
            changes: (0..budget.changes * 4)
                .map(|i| json!({"version": format!("0.{}.0", 100 + i), "date": "2026-10-19", "summary": long('s', 300)}))
                .collect(),
            tools: (0..50)
                .map(|i| ToolEntry {
                    name: format!("{}{i:02}", long('t', 39)),
                    kind: "python".to_string(),
                    last_call: Some(LastCall { at: 1_760_000_000 + i, ok: false, error_class: Some("tool_exception".to_string()) }),
                })
                .collect(),
            runs: (0..budget.runs)
                .map(|i| RunEntry {
                    id: format!("01J{}", long('R', 23)),
                    tool: format!("{}{i:02}", long('t', 39)),
                    status: "running".to_string(),
                    started: Some(1_760_000_000),
                })
                .collect(),
            notes: (0..1000)
                .map(|i| NoteEntry {
                    key: format!("notes/01J{}{i:03}", long('N', 20)),
                    text: long('x', 4096),
                    attached_to: Some(Attach::Tool(long('t', 41))),
                })
                .collect(),
        }
    }

    #[test]
    fn worst_case_renders_within_the_budget_and_names_every_cut() {
        let v = render(&worst_case(&BUDGET), &BUDGET);
        let bytes = serde_json::to_vec(&v).unwrap().len();
        assert!(bytes <= BUDGET.max_bytes, "{bytes} bytes");
        assert_eq!(v["truncated"]["tools"], true);
        assert_eq!(v["truncated"]["notes"], true);
        assert_eq!(v["truncated"]["changes"], true);
        assert_eq!(v["next_call"]["tool"], "host.tool.test");
    }

    #[test]
    fn a_small_tenant_renders_uncut() {
        let snap = Snapshot {
            namespace: "acme".to_string(),
            last_seen: None,
            version: "0.112.0".to_string(),
            version_at_last_call: None,
            changes: vec![],
            tools: vec![ToolEntry { name: "a".to_string(), kind: "echo".to_string(), last_call: None }],
            runs: vec![],
            notes: vec![],
        };
        let v = render(&snap, &BUDGET);
        assert_eq!(v["truncated"], json!({"tools": false, "notes": false, "runs_open": false, "changes": false}));
        assert_eq!(v["next_call"]["tool"], "host.tool.test");
        assert_eq!(v["next_call"]["args"], json!({"name": "a"}));
    }

    #[test]
    fn sessions_split_on_gaps_and_count_the_first_host_call() {
        let rows = |v: &[(i64, &str)]| v.iter().map(|(t, n)| (*t, n.to_string())).collect::<Vec<_>>();
        // Two sessions: one opens with a user tool then host.resume (resume is its
        // first host.* call), one opens with another host.* call.
        let r = rows(&[(0, "echo"), (60, "host.resume"), (5000, "host.tool.list"), (5060, "host.resume")]);
        assert_eq!(count_first_calls(&r), (2, 1));
        assert_eq!(count_first_calls(&[]), (0, 0));
    }

    #[test]
    fn every_state_has_exactly_one_row() {
        for state in ResumeState::ALL {
            assert_eq!(RESUME_NEXT.iter().filter(|r| r.0 == state).count(), 1, "{state:?}");
            assert_eq!(next_row(state).0, state);
        }
    }
}
