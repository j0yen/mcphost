//! PRD-mcphost-refusals-name-the-nearest-move: the pure half of the
//! tenant-path `tool_not_found` -- the token-aware name scorer, the candidate
//! ranking over the caller-visible name sets, and the `next_call` chosen from
//! the miss's own facts. `handler.rs` gathers the sets (the caller's own
//! tools, the shares it can call, the exact catalog count, the canonical
//! control-plane names) and hands them here; nothing in this file reads a
//! database, so a candidate can only come from a set the caller could already
//! list.

use serde_json::{Map, Value, json};

use crate::errors::{MAX_DID_YOU_MEAN, MAX_DID_YOU_MEAN_DISTANCE, levenshtein};

/// `data.nearest`'s cap (no distance cutoff, so it is never empty while any
/// candidate exists).
pub const MAX_NEAREST: usize = 5;

/// The host verbs a misnamed tool must never be steered into by a hint that
/// replays the caller's arguments: both act on the credential itself.
const DESTRUCTIVE_VERBS: &[&str] = &["host.key.rotate", "host.key_rotate", "host.self.offboard", "host.self_offboard"];

/// Where a candidate name came from; the order is the order `next_call` is
/// chosen in (own tool, then a share, then a control-plane verb).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Group {
    Own,
    Shared,
    Control,
}

/// The caller-visible name sets one miss is scored against.
#[derive(Debug, Clone, Copy, Default)]
pub struct Universe<'a> {
    /// The caller's own published tool names.
    pub own: &'a [String],
    /// `<owner_ns>.<tool>` names shared with the caller.
    pub shared: &'a [String],
    /// The control-plane verbs, spelled the way a client calls them.
    pub control: &'a [String],
}

/// What a tenant-path `tool_not_found` carries beyond its message.
#[derive(Debug, Clone, PartialEq)]
pub struct Near {
    /// Up to [`MAX_NEAREST`] names, closest first, no distance cutoff.
    pub nearest: Vec<String>,
    /// Names within [`MAX_DID_YOU_MEAN_DISTANCE`] by [`score`].
    pub did_you_mean: Vec<String>,
    /// The caller's own tool count.
    pub tools_published: i64,
    /// Exact-name catalog hits; `None` when the lookup failed.
    pub catalog_matches: Option<i64>,
    /// `{tool, args}` chosen from the facts above.
    pub next_call: Value,
    /// A static hint a site wants kept in `data` (the shared-miss text).
    pub hint: Option<&'static str>,
}

impl Near {
    /// The additive `data` keys. `next_call` is not written here: the one
    /// envelope point in `errors.rs` writes it, reading [`Near::next_call`].
    pub fn insert_into(&self, obj: &mut Map<String, Value>) {
        obj.insert("nearest".to_string(), json!(self.nearest));
        obj.insert("did_you_mean".to_string(), json!(self.did_you_mean));
        obj.insert("tools_published".to_string(), json!(self.tools_published));
        if let Some(n) = self.catalog_matches {
            obj.insert("catalog_matches".to_string(), json!(n));
        }
        if let Some(h) = self.hint {
            obj.insert("hint".to_string(), json!(h));
        }
    }
}

/// Lower-cased name tokens split on `_` `.` `-`, the `host`/`billing`
/// namespace words dropped (every control-plane verb carries one).
fn tokens(name: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in name.to_ascii_lowercase().split(['_', '.', '-']) {
        if t.is_empty() || t == "host" || t == "billing" || out.iter().any(|o| o == t) {
            continue;
        }
        out.push(t.to_string());
    }
    out
}

/// Token-aware distance: the edit distance over the whole name, or -- when
/// the two names share at least two tokens -- the count of tokens that do not
/// pair up, whichever is smaller. `publish_agent_tool` is one token from
/// `host.tool_publish` although it is eight edits away.
pub fn score(requested: &str, candidate: &str) -> usize {
    let whole = levenshtein(&requested.to_ascii_lowercase(), &candidate.to_ascii_lowercase());
    let (a, b) = (tokens(requested), tokens(candidate));
    let shared = a.iter().filter(|t| b.contains(t)).count();
    if shared >= 2 {
        whole.min((a.len() - shared).max(b.len() - shared))
    } else {
        whole
    }
}

/// `args` as the caller sent them with the reserved `tenant_key` removed, so
/// a replayed call never carries the credential.
pub fn sent_args(args: Option<&Value>) -> Value {
    match args {
        Some(Value::Object(m)) => {
            let mut m = m.clone();
            m.remove("tenant_key");
            Value::Object(m)
        }
        _ => json!({}),
    }
}

/// Builds the refusal facts for `requested`. `sent` is the call's own
/// arguments (already free of `tenant_key`, see [`sent_args`]).
pub fn build(
    requested: &str,
    sent: &Value,
    universe: &Universe<'_>,
    tools_published: i64,
    catalog_matches: Option<i64>,
) -> Near {
    let mut cands: Vec<(Group, usize, &str)> = Vec::new();
    for (group, names) in [(Group::Own, universe.own), (Group::Shared, universe.shared), (Group::Control, universe.control)] {
        for n in names {
            if !cands.iter().any(|(_, _, c)| c == n) {
                cands.push((group, score(requested, n), n.as_str()));
            }
        }
    }
    let mut hits: Vec<&(Group, usize, &str)> = cands.iter().filter(|(_, d, _)| *d <= MAX_DID_YOU_MEAN_DISTANCE).collect();
    hits.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(b.2)));
    hits.truncate(MAX_DID_YOU_MEAN);
    let did_you_mean: Vec<String> = hits.iter().map(|(_, _, n)| n.to_string()).collect();

    let mut rest: Vec<&(Group, usize, &str)> = cands.iter().collect();
    rest.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(b.2)));
    let mut nearest = did_you_mean.clone();
    for (_, _, n) in rest {
        if nearest.len() >= MAX_NEAREST {
            break;
        }
        if !nearest.iter().any(|x| x == n) {
            nearest.push(n.to_string());
        }
    }

    let next_call = if let Some((_, _, n)) = hits.iter().find(|(g, _, _)| *g == Group::Own) {
        json!({"tool": "host.tool_call", "args": {"name": n, "args": sent}})
    } else if hits.iter().any(|(g, _, _)| *g == Group::Shared) {
        json!({"tool": "host.tool.list_shared", "args": {}})
    } else if let Some((_, _, n)) =
        hits.iter().find(|(g, _, n)| *g == Group::Control && !DESTRUCTIVE_VERBS.contains(n))
    {
        json!({"tool": n, "args": sent})
    } else if catalog_matches.is_some_and(|n| n > 0) {
        json!({"tool": "host.catalog.search", "args": {"q": requested}})
    } else {
        json!({"tool": "host.tool_publish", "args": {"name": requested}})
    };

    Near { nearest, did_you_mean, tools_published, catalog_matches, next_call, hint: None }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_token_reordering_scores_one() {
        assert_eq!(score("publish_agent_tool", "host.tool_publish"), 1);
        assert!(score("agent_self_register", "host.tool_publish") > MAX_DID_YOU_MEAN_DISTANCE);
    }

    #[test]
    fn own_tool_leads_and_names_the_call() {
        let own = names(&["tide_report", "tide_summary"]);
        let control = names(&["host.tool_publish"]);
        let u = Universe { own: &own, shared: &[], control: &control };
        let near = build("tide_reprot", &json!({"port": "sfo"}), &u, 2, Some(0));
        assert_eq!(near.did_you_mean, ["tide_report"]);
        assert_eq!(near.nearest[..2], ["tide_report", "tide_summary"]);
        assert_eq!(
            near.next_call,
            json!({"tool": "host.tool_call", "args": {"name": "tide_report", "args": {"port": "sfo"}}})
        );
    }

    #[test]
    fn empty_tenant_publishes_under_the_requested_name() {
        let control = names(&["host.tool_publish", "signup"]);
        let u = Universe { own: &[], shared: &[], control: &control };
        let near = build("agent_self_register", &json!({}), &u, 0, Some(0));
        assert!(near.did_you_mean.is_empty());
        assert!(!near.nearest.is_empty());
        assert_eq!(near.next_call, json!({"tool": "host.tool_publish", "args": {"name": "agent_self_register"}}));
    }

    #[test]
    fn destructive_verb_is_listed_but_never_replayed() {
        let control = names(&["host.key_rotate"]);
        let u = Universe { own: &[], shared: &[], control: &control };
        let near = build("key_rotate", &json!({"x": 1}), &u, 0, None);
        assert_eq!(near.did_you_mean, ["host.key_rotate"]);
        assert_eq!(near.next_call["tool"], "host.tool_publish");
    }

    #[test]
    fn catalog_hit_beats_publish() {
        let u = Universe::default();
        let near = build("zzz", &json!({}), &u, 0, Some(1));
        assert_eq!(near.next_call, json!({"tool": "host.catalog.search", "args": {"q": "zzz"}}));
    }

    #[test]
    fn sent_args_drops_the_credential() {
        assert_eq!(sent_args(Some(&json!({"a": 1, "tenant_key": "k"}))), json!({"a": 1}));
        assert_eq!(sent_args(None), json!({}));
    }
}
