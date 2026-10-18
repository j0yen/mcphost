//! The `chain` kind: `spec` is an ordered list of steps, each naming a tool
//! in the same tenant and a mapping from prior results (and the chain's own
//! call args) into that step's arguments. A call runs every step in order,
//! host-side (no sandbox involved for the chain itself -- see
//! `kinds::compose_call`, which this module's step dispatch is built on).
//!
//! PRD-mcphost-composition requirements 2/3/4 (this is an iter-1 scaffold,
//! not the whole PRD -- see the module doc below for what's covered and
//! what's deferred):
//!
//! - Requirement 2's ceilings (depth, self-call, children) are enforced by
//!   [`super::compose_call`], which every step dispatch here goes through.
//! - Requirement 3's `chain` kind: `steps: [{tool, args, on_error?}]`,
//!   published like any tool, `host.tool_test` dry-runs it (this kind
//!   special-cases `ctx.test_mode` rather than actually dispatching any
//!   step -- see [`ChainKind::call`]'s test-mode branch).
//! - Requirement 4's mapping grammar (`$`, dotted keys, integer indices) is
//!   [`super::Path`] verbatim -- a step's `args` map each own-argument name
//!   to either a literal JSON value or (a string starting with `"$."`) a
//!   path resolved against `{"input": <chain's own args>, "prev": {"result":
//!   ...}, "steps": [{"result": ...}, ...]}`.
//!
//! Deferred to a later tick (documented, not silently dropped):
//! - (none for `outputs`: the universal `outputs` spec field is applied --
//!   see [`project_outputs`].)
//! - `on_error: "continue"` (requirement 3) is implemented (requirement 3 /
//!   AC7); `map` steps (P2) are not.
//!
//! PRD-mcphost-chain-run-lineage (requirements 1-3): real child-run rows
//! (`host.runs.get` inlining children, `trigger: "composition"`) are now
//! written by [`super::compose_call`] itself for every step dispatch below -- this
//! kind's own `steps` trace stays (existing callers of the chain's result
//! shape are unaffected), it's no longer the only record of what ran.
//! `describe()` now derives `input_schema.required` from every step's
//! `$.input.<name>` mapping paths, and [`ChainKind::call`] refuses a call
//! missing one of them (`compose_input_missing`) before step 1 ever
//! dispatches -- see [`super::Kind::validates_own_args`]'s own doc comment
//! for why the generic dispatch-time schema check has to step aside for
//! this kind specifically.

use serde_json::{Map, Value, json};

use super::contract::{
    CASE_BUDGET_EXCEEDED, CASE_CALL, CaseSkip, COMPOSE_FAMILY, ConformanceFixtures, KindContract, SourcePosition, TOOL_STATE_FAMILY, codes,
};
use super::{
    CallCtx, HOST_STEPS_ALLOWED, Kind, KindError, KindExample, KindRegistry, OutputDecl, Path, ToolDescriptor,
    apply_output_decls, compose_call_ref, spec_declared_outputs,
};
use crate::errors::AppError;

pub struct ChainKind;

/// The universal `outputs` spec field on a chain: `None` when nothing is
/// declared (the call result is unchanged), else the `payload` map
/// `result.payload.<field>` reads. A path entry (`$.steps[0].result.x`,
/// `$.input.x`, `$.prev.result.x`) resolves against the chain's own
/// [`step_context`] after the last step; a bare name is promoted from the
/// last step's result by the same any-wrapper search python/wasm use. The
/// map starts as a mirror of the last step's result object (the "payload
/// mirrors body" shape every kind shares).
fn project_outputs(
    declared: &[OutputDecl],
    input: &Value,
    prev: Option<&Value>,
    step_results: &[Value],
    last: &Value,
) -> Option<Value> {
    if declared.is_empty() {
        return None;
    }
    let (with_path, bare): (Vec<OutputDecl>, Vec<OutputDecl>) =
        declared.iter().cloned().partition(|d| d.path.is_some());
    let mut payload = last.as_object().cloned().unwrap_or_default();
    apply_output_decls(&mut payload, &step_context(input, prev, step_results), &with_path, None);
    apply_output_decls(&mut payload, last, &bare, None);
    Some(Value::Object(payload))
}

// CONTEXT_KEYS-BEGIN
/// PRD-mcphost-chain-publish-time-path-check requirement 5: the ONE table of
/// context keys. [`step_context`] (the constructor) and every checker below
/// read the names from here; nothing else in this file spells them, and
/// `tests/cpath_ac07_context_keys_table_parity.rs` scans the file to keep it
/// that way.
pub struct ContextKeys {
    /// Top level: the chain call's own args.
    pub input: &'static str,
    /// Top level: `{result}` of the previous step, or null.
    pub prev: &'static str,
    /// Top level: `[{result}, ...]` of every earlier step.
    pub steps: &'static str,
    /// Top level: the delivery when an event trigger fired the run, else null.
    pub event: &'static str,
    /// The one key under `prev` / `steps[i]`.
    pub result: &'static str,
    /// PRD-mcphost-chain-fanout-and-join R1: inside a `for_each` step only,
    /// the current element of the fanned-out array.
    pub item: &'static str,
    /// Inside a `for_each` step only, the element's 0-based position.
    pub index: &'static str,
}

pub const CONTEXT_KEYS: ContextKeys =
    ContextKeys { input: "input", prev: "prev", steps: "steps", event: "event", result: "result", item: "item", index: "index" };
// CONTEXT_KEYS-END

/// Requirement 4 ("up to three nearest sibling names"): the closest
/// `tenant_names` to `name` by plain edit distance, nearest first, capped
/// at three -- no distance cutoff (unlike `errors::did_you_mean`'s kind-name
/// suggestions), since any three of a tenant's own tool names are a useful
/// pointer for a typo'd step, not only a near-miss one.
fn nearest_sibling_names(name: &str, tenant_names: &[String]) -> Vec<String> {
    let mut scored: Vec<(usize, &str)> = tenant_names
        .iter()
        .map(|n| (crate::errors::levenshtein(name, n), n.as_str()))
        .collect();
    scored.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(b.1)));
    scored.into_iter().take(3).map(|(_, n)| n.to_string()).collect()
}

/// Requirement 4: resolves every step's `tool` against `tenant_id`'s own
/// tools, then [`HOST_STEPS_ALLOWED`] -- called once at `host.tool_publish`
/// and `host.spec_test` time (never from [`super::compose_call`]'s own, narrower,
/// runtime-only resolution), so an unresolvable step is refused before
/// anything is published rather than discovered the first time a trigger
/// fires (this PRD's own grounding incident: a chain with a bare
/// `host.table.append` step published and dry-ran green, then failed every
/// real fire). A bare name that exists as neither fails `step_tool_not_found`
/// (`data.step`, `data.name`, up to three `data.suggestions`); a `host.*`
/// name off the allowlist fails `step_tool_not_allowed` (`data.allowed`).
/// `data.step` is 1-based, matching this module's own `steps`/`steps_trace`
/// numbering everywhere else (`dry_run_report`, `failed_step`).
pub async fn resolve_steps(db: &crate::db::Db, tenant_id: i64, spec: &Value) -> Result<(), AppError> {
    let steps = parse_steps(spec).map_err(AppError::from)?;
    let mut tenant_names: Option<Vec<String>> = None;
    for (i, step) in steps.iter().enumerate() {
        let step_no = i + 1;
        if db.get_tool(tenant_id, step.tool.clone()).await?.is_some() {
            continue;
        }
        if HOST_STEPS_ALLOWED.contains(&step.tool.as_str()) {
            continue;
        }
        if step.tool.starts_with("host.") || step.tool.starts_with("billing.") {
            return Err(AppError::Structured {
                code: STEP_TOOL_NOT_ALLOWED,
                message: format!(
                    "step {step_no} ('{}'): not an allowlisted host step; allowed: {}",
                    step.tool,
                    HOST_STEPS_ALLOWED.join(", ")
                ),
                data: json!({"step": step_no, "name": step.tool, "allowed": HOST_STEPS_ALLOWED}),
            });
        }
        if tenant_names.is_none() {
            let names = db.list_tools(tenant_id).await?.into_iter().map(|t| t.name).collect();
            tenant_names = Some(names);
        }
        let suggestions = nearest_sibling_names(&step.tool, tenant_names.as_deref().unwrap_or(&[]));
        return Err(AppError::Structured {
            code: STEP_TOOL_NOT_FOUND,
            message: format!("step {step_no} ('{}'): no such tool", step.tool),
            data: json!({"step": step_no, "name": step.tool, "suggestions": suggestions}),
        });
    }
    Ok(())
}

/// PRD-mcphost-event-trigger-from-webhook R6: the first-level roots a step
/// mapping may read (`$.input`, `$.prev`, `$.steps[i]`, `$.event`) with what
/// each holds -- the one table [`step_context`]'s keys are asserted against,
/// so a root cannot be added to the context without being named here.
pub const CONTEXT_ROOTS: &[(&str, &str)] = &[
    (CONTEXT_KEYS.input, "this call's own args"),
    (CONTEXT_KEYS.prev, "the previous step's result, as {result}, or null on step 1"),
    (CONTEXT_KEYS.steps, "every earlier step's result, as [{result}, ...]"),
    (
        CONTEXT_KEYS.event,
        "the delivery (headers, body, received_unix) when an event trigger fired this run, else null",
    ),
];

tokio::task_local! {
    /// Set by the run executor for a run an event or webhook trigger fired,
    /// so [`step_context`] can tell a delivery from a manual call whose args
    /// merely contain an `event` key.
    static EVENT_FIRED: bool;
}

/// Runs `fut` with `$.event` live (`fired`) or null.
pub async fn with_event_fired<F: std::future::Future>(fired: bool, fut: F) -> F::Output {
    EVENT_FIRED.scope(fired, fut).await
}

/// PRD-mcphost-chain-prev-contract P0 requirement 1: the ONE constructor of
/// the context a step's mappings resolve against -- `{"input": <chain args>,
/// "prev": {"result": <predecessor result>} | null, "steps": [{"result":
/// ...}, ...], "event": <delivery> | null}`. [`ChainKind::call`] and
/// [`dry_run_report`] both build their context here, and
/// [`accepted_prev_keys`]/[`accepted_step_keys`] read the checker's accepted
/// first-level keys back out of a context this function built, so the two
/// cannot drift apart.
pub fn step_context(input: &Value, prev: Option<&Value>, steps: &[Value]) -> Value {
    let event = if EVENT_FIRED.try_with(|fired| *fired).unwrap_or(false) {
        input.get(CONTEXT_KEYS.event).cloned().unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    json!({
        (CONTEXT_KEYS.input): input,
        (CONTEXT_KEYS.prev): prev.map(|r| json!({ (CONTEXT_KEYS.result): r })),
        (CONTEXT_KEYS.steps): steps.iter().map(|r| json!({ (CONTEXT_KEYS.result): r })).collect::<Vec<_>>(),
        (CONTEXT_KEYS.event): event,
    })
}

/// PRD-mcphost-chain-fanout-and-join R1: `base` (a [`step_context`] result)
/// plus the `for_each` step's `$.item` and `$.index`.
fn item_context(base: &Value, item: &Value, index: usize) -> Value {
    let mut ctx = base.clone();
    if let Value::Object(map) = &mut ctx {
        map.insert(CONTEXT_KEYS.item.to_string(), item.clone());
        map.insert(CONTEXT_KEYS.index.to_string(), json!(index));
    }
    ctx
}

/// `raw` is a mapping into `$.item` or `$.index` (the root or a path under it).
fn is_item_ref(raw: &str) -> bool {
    [CONTEXT_KEYS.item, CONTEXT_KEYS.index].iter().any(|key| {
        raw.strip_prefix("$.")
            .and_then(|rest| rest.strip_prefix(key))
            .is_some_and(|tail| tail.is_empty() || tail.starts_with(['.', '[']))
    })
}

fn object_keys(v: &Value) -> std::collections::BTreeSet<String> {
    v.as_object().map(|m| m.keys().cloned().collect()).unwrap_or_default()
}

/// The first-level keys a mapping may use under `$.prev` (today `{result}`),
/// read from a sample context [`step_context`] built.
pub fn accepted_prev_keys() -> std::collections::BTreeSet<String> {
    object_keys(&step_context(&Value::Null, Some(&Value::Null), &[])[CONTEXT_KEYS.prev])
}

/// The first-level keys a mapping may use under `$.steps[i]` (today
/// `{result}`), read from a sample context [`step_context`] built.
pub fn accepted_step_keys() -> std::collections::BTreeSet<String> {
    object_keys(&step_context(&Value::Null, None, &[Value::Null])[CONTEXT_KEYS.steps][0])
}

fn keys_phrase(keys: &std::collections::BTreeSet<String>) -> String {
    let names = keys.iter().map(|k| format!("`{k}`")).collect::<Vec<_>>().join(", ");
    match keys.len() {
        1 => format!("exactly one key, {names}"),
        n => format!("exactly {n} keys, {names}"),
    }
}

/// PRD-mcphost-chain-prev-contract P1 requirement 7: the one-sentence
/// statement of the mapping grammar, rendered from the derived key sets
/// ([`accepted_prev_keys`]/[`accepted_step_keys`]) -- used verbatim by
/// `docs/kinds/chain.md` and [`ChainKind::example`], and checked by a test.
pub fn grammar_sentence() -> String {
    format!(
        "`$.prev` has {}; `$.steps[i]` has {}.",
        keys_phrase(&accepted_prev_keys()),
        keys_phrase(&accepted_step_keys())
    )
}

/// One `$.prev...`/`$.steps[i]...` mapping split for checking: which step it
/// reads (1-based, same numbering as everywhere else in this module), the
/// `prefix` that named it (`$.prev` or `$.steps[i]`), and the raw path
/// `tokens` after it (`.name` or `[idx]`, in order). Deliberately raw-string
/// parsing (like [`input_arg_name`] below), not [`Path::parse`] -- a malformed
/// path is `resolve_args`'s own call-time error to report, not a
/// publish/test-time one. `None` for a literal, a `$.input.*` path, or a
/// malformed `$.steps[...]` index.
struct StepRef {
    predecessor_no: usize,
    is_prev: bool,
    prefix: String,
    tokens: Vec<String>,
}

impl StepRef {
    fn parse(raw: &str, this_step_no: usize) -> Option<StepRef> {
        if let Some(rest) = raw.strip_prefix("$.prev") {
            let predecessor_no = this_step_no.checked_sub(1).filter(|n| *n > 0)?;
            return Some(StepRef {
                predecessor_no,
                is_prev: true,
                prefix: "$.prev".to_string(),
                tokens: path_tokens(rest),
            });
        }
        let rest = raw.strip_prefix("$.steps[")?;
        let end = rest.find(']')?;
        let idx: usize = rest[..end].parse().ok()?;
        Some(StepRef {
            predecessor_no: idx + 1,
            is_prev: false,
            prefix: format!("$.steps[{idx}]"),
            tokens: path_tokens(&rest[end + 1..]),
        })
    }

    /// The name of token `i` when it is a dotted `.name` segment.
    fn name(&self, i: usize) -> Option<&str> {
        self.tokens.get(i)?.strip_prefix('.')
    }

    /// The accepted first-level keys under this reference's prefix, read
    /// from the context constructor ([`step_context`]).
    fn accepted_keys(&self) -> std::collections::BTreeSet<String> {
        if self.is_prev { accepted_prev_keys() } else { accepted_step_keys() }
    }

    /// The predecessor's actual result in `context` this reference reads
    /// from (`None` when that step never produced one).
    fn base_result<'a>(&self, context: &'a Value) -> Option<&'a Value> {
        let holder = if self.is_prev {
            &context[CONTEXT_KEYS.prev]
        } else {
            &context[CONTEXT_KEYS.steps][self.predecessor_no - 1]
        };
        holder.get(CONTEXT_KEYS.result).filter(|v| !v.is_null())
    }

    /// This path rebuilt with the `hop` names inserted ahead of every
    /// token, and token `at.0` (when given) replaced by the dotted name
    /// `at.1`.
    fn rebuilt(&self, hop: &[&str], at: Option<(usize, &str)>) -> String {
        let mut out = self.prefix.clone();
        for h in hop {
            out.push('.');
            out.push_str(h);
        }
        for (i, t) in self.tokens.iter().enumerate() {
            match at {
                Some((idx, name)) if idx == i => {
                    out.push('.');
                    out.push_str(name);
                }
                _ => out.push_str(t),
            }
        }
        out
    }
}

/// Splits the remainder of a path after its `$.prev`/`$.steps[i]` prefix
/// into `.name` / `[idx]` tokens. Stops (returns what it has) at anything
/// that is neither -- a malformed tail is `resolve_args`'s to report.
fn path_tokens(mut rest: &str) -> Vec<String> {
    let mut out = Vec::new();
    while !rest.is_empty() {
        let end = if let Some(tail) = rest.strip_prefix('.') {
            tail.find(['.', '[']).map_or(rest.len(), |e| e + 1)
        } else if rest.starts_with('[') {
            match rest.find(']') {
                Some(e) => e + 1,
                None => break,
            }
        } else {
            break;
        };
        if end <= 1 {
            break;
        }
        out.push(rest[..end].to_string());
        rest = &rest[end..];
    }
    out
}

/// The nearest of `known` to `name` by edit distance, only when it is
/// close enough to be a plausible typo (distance <= 2, e.g. `lead` ->
/// `leads`).
fn closest_name<'a>(name: &str, known: &'a [String]) -> Option<&'a str> {
    known
        .iter()
        .map(|k| (crate::errors::levenshtein(name, k), k.as_str()))
        .filter(|(d, _)| *d <= 2)
        .min_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(b.1)))
        .map(|(_, k)| k)
}

/// Requirement 2: the declared output field names of tenant tool
/// `tool_name` ([`super::Kind::declared_outputs`], e.g. an `http`/`python`/
/// `wasm` spec's own `outputs`) -- `None` when `tool_name` isn't one of
/// this tenant's own published tools (a bare lookup miss, or an
/// allowlisted `host.*` verb, which has no stored spec at all) or its kind
/// is unregistered; `Some(vec![])` when it resolves but declares no
/// outputs (`echo`, or any kind's spec with an empty/absent `outputs`).
/// The two `None`/`Some(empty)` cases are deliberately NOT collapsed --
/// [`check_step_ref`] treats both as "no output schema," but keeps them as
/// separate match arms for whichever future caller wants to tell "unknown
/// tool" apart from "known tool, declares nothing."
async fn declared_output_names(
    db: &crate::db::Db,
    kinds: &KindRegistry,
    tenant_id: i64,
    tool_name: &str,
) -> Option<Vec<String>> {
    let row = db.get_tool(tenant_id, tool_name.to_string()).await.ok()??;
    let kind = kinds.get(&row.kind)?;
    Some(kind.declared_outputs(&row.spec).into_iter().map(|d| d.name).collect())
}

/// Requirement 2: how one `$.prev`/`$.steps[i]` mapping into
/// `predecessor_tool` classifies. `db`/`kinds` are `None` whenever a
/// caller has no descriptor lookup available at all ([`CallCtx::for_test`]'s
/// own `compose_db`/`compose_kinds`, left unset there) -- that degrades to
/// `Unverifiable`, same as a lookup miss, rather than panicking or
/// guessing.
///
/// PRD-mcphost-chain-prev-contract P0 requirement 2: the first segment must
/// be one of the context constructor's own keys ([`StepRef::accepted_keys`],
/// today `result`); a missing hop is `WillFail` with a corrected
/// `did_you_mean` whether or not the predecessor declares anything. The
/// segment after the hop is then checked against the predecessor's declared
/// `outputs` (and the envelope wrapper keys) when it declares any.
enum RefCheck {
    Pass,
    /// Extra evidence fields (`available`, `predecessor_keys`,
    /// `did_you_mean`) merged into the failure entry.
    WillFail(Map<String, Value>),
    Unverifiable,
}

/// PRD-mcphost-chain-publish-time-path-check requirement 1: a JSON value's
/// type name as recorded in `tools.observed_outputs_json`.
fn json_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// PRD-mcphost-chain-publish-time-path-check requirement 1: the observed
/// shape of one run result -- top-level keys of an object result mapped to
/// their JSON type, plus, for an object-valued key, its own keys one level
/// down as `"<key>.<sub>"`. Empty for a non-object result. Capped at
/// [`crate::db::OBSERVED_KEYS_CAP`] keys.
pub fn observed_shape(result: &Value) -> std::collections::BTreeMap<String, &'static str> {
    let mut out = std::collections::BTreeMap::new();
    let Some(map) = result.as_object() else {
        return out;
    };
    for (k, v) in map {
        out.insert(k.clone(), json_type(v));
        if let Value::Object(sub) = v {
            for (sk, sv) in sub {
                out.insert(format!("{k}.{sk}"), json_type(sv));
            }
        }
    }
    while out.len() > crate::db::OBSERVED_KEYS_CAP {
        out.pop_last();
    }
    out
}

/// What a predecessor's `$.prev.result.<key>` mapping is checked against:
/// its declared `outputs` when it has any, else the keys its finished runs
/// actually returned.
struct PathBasis {
    /// Top-level key names, sorted for `observed`, declaration order for
    /// `declared`.
    names: Vec<String>,
    /// Of those, the observed ones whose value was a JSON array (the
    /// `did_you_mean` fallback when no name is a near typo).
    array_names: Vec<String>,
    source: &'static str,
    /// Observed JSON type of each top-level key (empty without run history).
    types: std::collections::BTreeMap<String, String>,
    /// `Some` whenever the tool has at least one recorded run.
    runs_sampled: Option<i64>,
    /// Declared names no run ever returned (empty unless `declared` and
    /// there is observed history).
    drift: Vec<String>,
}

async fn path_basis(
    db: &crate::db::Db,
    kinds: &KindRegistry,
    tenant_id: i64,
    tool: &str,
    use_observed: bool,
) -> Option<PathBasis> {
    let declared = declared_output_names(db, kinds, tenant_id, tool).await.unwrap_or_default();
    let observed = if use_observed {
        db.get_observed_outputs(tenant_id, tool.to_string()).await.ok().flatten()
    } else {
        None
    };
    let observed_top: Vec<String> = observed
        .as_ref()
        .map(|o| o.keys.keys().filter(|k| !k.contains('.')).cloned().collect())
        .unwrap_or_default();
    let runs_sampled = observed.as_ref().map(|o| o.runs_sampled);
    let types: std::collections::BTreeMap<String, String> = observed
        .as_ref()
        .map(|o| o.keys.iter().filter(|(k, _)| !k.contains('.')).map(|(k, t)| (k.clone(), t.clone())).collect())
        .unwrap_or_default();
    if !declared.is_empty() {
        let drift = if observed.is_some() {
            declared.iter().filter(|d| !observed_top.contains(d)).cloned().collect()
        } else {
            Vec::new()
        };
        return Some(PathBasis { names: declared, array_names: Vec::new(), source: "declared", types, runs_sampled, drift });
    }
    let o = observed?;
    if observed_top.is_empty() {
        return None;
    }
    let array_names = observed_top
        .iter()
        .filter(|k| o.keys.get(*k).is_some_and(|t| t == "array"))
        .cloned()
        .collect();
    Some(PathBasis { names: observed_top, array_names, source: "observed", types, runs_sampled, drift: Vec::new() })
}

impl PathBasis {
    /// The key to suggest for a missing `name`: a near typo among the
    /// names, else -- for an observed shape -- its one array-valued key
    /// (an agent asking for `rows` of a `{items, count}` result wants the
    /// collection).
    fn suggest(&self, name: &str) -> Option<String> {
        if self.names.iter().any(|d| d == name) {
            return None;
        }
        if let Some(near) = closest_name(name, &self.names) {
            return Some(near.to_string());
        }
        match self.array_names.as_slice() {
            [only] => Some(only.clone()),
            _ => None,
        }
    }
}

/// The observed JSON type of the one field `r` names directly under the
/// predecessor's result (`$.prev.result.<field>`); `None` for a deeper or
/// whole-result path, or when no run ever returned the field.
#[allow(clippy::too_many_arguments)]
async fn observed_field_type(
    db: &crate::db::Db,
    kinds: &KindRegistry,
    tenant_id: i64,
    predecessor_tool: &str,
    r: &StepRef,
    use_observed: bool,
) -> Option<String> {
    if r.tokens.len() != 2 || !r.accepted_keys().contains(r.name(0)?) {
        return None;
    }
    let basis = path_basis(db, kinds, tenant_id, predecessor_tool, use_observed).await?;
    basis.types.get(r.name(1)?).cloned()
}

#[allow(clippy::too_many_arguments)]
async fn check_step_ref(
    db: Option<&crate::db::Db>,
    kinds: Option<&KindRegistry>,
    tenant_id: i64,
    predecessor_tool: &str,
    r: &StepRef,
    use_observed: bool,
) -> RefCheck {
    let basis = match (db, kinds) {
        (Some(db), Some(kinds)) => path_basis(db, kinds, tenant_id, predecessor_tool, use_observed).await,
        _ => None,
    };
    let mut evidence = Map::new();
    if let Some(b) = &basis {
        evidence.insert("available".to_string(), json!(b.names));
        evidence.insert("predecessor_keys".to_string(), json!(b.names));
        if b.source == "observed" || b.runs_sampled.is_some() {
            evidence.insert("prev_fields_source".to_string(), json!(b.source));
        }
        if let Some(n) = b.runs_sampled {
            evidence.insert("runs_sampled".to_string(), json!(n));
        }
    }
    let Some(first) = r.name(0) else {
        return RefCheck::Unverifiable;
    };
    if !r.accepted_keys().contains(first) {
        // Missing hop: `$.prev.<seg>` should be `$.prev.result.<seg>`; when
        // the predecessor's fields name a close match for `<seg>`, point at
        // that field instead.
        let suggestion = match basis.as_ref().and_then(|b| b.suggest(first)) {
            Some(near) => r.rebuilt(&[CONTEXT_KEYS.result], Some((0, &near))),
            None => r.rebuilt(&[CONTEXT_KEYS.result], None),
        };
        evidence.insert("did_you_mean".to_string(), json!(suggestion));
        return RefCheck::WillFail(evidence);
    }
    let Some(field) = r.name(1) else {
        // Whole predecessor value (`$.prev.result`): nothing to check
        // against a schema; Pass only if the predecessor has one.
        return if basis.is_none() { RefCheck::Unverifiable } else { RefCheck::Pass };
    };
    let Some(basis) = basis else {
        return RefCheck::Unverifiable;
    };
    if basis.names.iter().any(|d| d == field) || super::ENVELOPE_WRAPPER_KEYS.contains(&field) {
        RefCheck::Pass
    } else {
        if let Some(near) = basis.suggest(field) {
            evidence.insert("did_you_mean".to_string(), json!(r.rebuilt(&[], Some((1, &near)))));
        }
        RefCheck::WillFail(evidence)
    }
}

/// PRD-mcphost-tool-test-truth P0 requirement 2 (AC1/AC2/AC3): the static,
/// call-args-independent verdict engine for a chain's own `$.prev`/
/// `$.steps[i]` mappings -- shared by [`verdict`] (`host.tool_publish`'s
/// gate, AC6) and [`dry_run_report`] (`host.tool_test`'s own dry run,
/// AC1-3), so the two never compute a different answer for the same spec.
/// Checks only the FIRST segment immediately after a `$.prev`/
/// `$.steps[i]` reference (Requirement 2's own words) against the
/// predecessor's declared output names -- a deliberately shallow,
/// schema-level check (Technical considerations), not a replay of the
/// real mapping grammar's deeper `$.prev.result.payload.<field>`
/// convention (unchanged, Non-goals). `will_fail` takes priority over
/// `unverifiable` when a spec has both, since a proven failure is more
/// actionable than "cannot tell."
struct StepsVerdict {
    /// `{"verdict": "pass"}`, `{"verdict": "will_fail", "failures":
    /// [{"step", "path", "predecessor_keys"}, ...]}` (every failing
    /// mapping, not just the first), or `{"verdict": "unverifiable",
    /// "reason": "no output schema for step <i>", "next": {"tool":
    /// "host_tool_call"}}` (the first one found).
    overall: Value,
    /// Every `(step_no, raw_path)` this check confirmed resolves to a real
    /// predecessor field -- [`dry_run_report`] skips the `unresolved_path`
    /// wrapper for exactly these (still no real VALUE to report without
    /// dispatching the step, but no longer claimed broken either).
    passes: std::collections::HashSet<(usize, String)>,
}

/// `$.event` or a path under it.
fn is_event_ref(raw: &str) -> bool {
    raw.strip_prefix("$.event")
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(['.', '[']))
}

async fn steps_verdict(
    db: Option<&crate::db::Db>,
    kinds: Option<&KindRegistry>,
    tenant_id: i64,
    steps: &[ParsedStep],
    use_observed: bool,
) -> StepsVerdict {
    let mut failures: Vec<Value> = Vec::new();
    let mut unverifiable_step: Option<usize> = None;
    let mut event_ref: Option<(usize, String)> = None;
    let mut passes = std::collections::HashSet::new();
    for (i, step) in steps.iter().enumerate() {
        let step_no = i + 1;
        // R5: a `for_each` path is a mapping too, and must read an array.
        let mappings = step
            .args
            .values()
            .filter_map(Value::as_str)
            .map(|raw| (raw.to_string(), false))
            .chain(step.for_each.clone().map(|raw| (raw, true)));
        for (raw, wants_array) in mappings {
            let raw = &raw;
            if is_event_ref(raw) && event_ref.is_none() {
                event_ref = Some((step_no, raw.clone()));
            }
            let Some(r) = StepRef::parse(raw, step_no) else {
                continue;
            };
            if r.tokens.is_empty() {
                continue; // whole-value mapping -- nothing to check.
            }
            let predecessor_no = r.predecessor_no;
            // PRD-mcphost-chain-publish-time-path-check requirement 4: a
            // `$.steps[i]` at or past the step's own position has no result
            // yet when the step runs.
            if predecessor_no >= step_no {
                failures.push(json!({
                    "step": step_no,
                    "path": raw,
                    "reason": "forward reference",
                    "available": [],
                }));
                continue;
            }
            let Some(predecessor) = steps.get(predecessor_no - 1) else {
                continue; // out of range -- not this check's concern.
            };
            let mut checked = check_step_ref(db, kinds, tenant_id, &predecessor.tool, &r, use_observed).await;
            if wants_array
                && !matches!(checked, RefCheck::WillFail(_))
                && let (Some(db), Some(kinds)) = (db, kinds)
                && let Some(actual) = observed_field_type(db, kinds, tenant_id, &predecessor.tool, &r, use_observed).await
                && actual != "array"
            {
                let mut evidence = Map::new();
                evidence.insert("expected".to_string(), json!("array"));
                evidence.insert("actual".to_string(), json!(actual));
                evidence.insert(
                    "reason".to_string(),
                    json!(format!("for_each needs an array but {raw} is {actual}")),
                );
                checked = RefCheck::WillFail(evidence);
            }
            match checked {
                RefCheck::Pass => {
                    passes.insert((step_no, raw.clone()));
                }
                RefCheck::WillFail(extra) => {
                    let mut entry = Map::new();
                    entry.insert("step".to_string(), json!(step_no));
                    entry.insert("path".to_string(), json!(raw));
                    entry.extend(extra);
                    failures.push(Value::Object(entry));
                }
                RefCheck::Unverifiable => {
                    if unverifiable_step.is_none() {
                        unverifiable_step = Some(predecessor_no);
                    }
                }
            }
        }
    }

    let overall = if !failures.is_empty() {
        json!({"verdict": "will_fail", "failures": failures, "evidence": failures})
    } else if let Some((step_no, path)) = event_ref {
        // R6: `$.event.*` exists only while a delivery fires the chain, so
        // publish cannot settle it.
        json!({
            "verdict": "unverifiable",
            "reason": format!(
                "step {step_no}: '{path}' resolves only when an event or webhook trigger fires \
                 this chain (trigger kind: event, delivery shape {{headers, body, received_unix}}); \
                 a manual call has $.event = null"
            ),
            "next": {"tool": "host_trigger_set", "kind": "event"},
        })
    } else if let Some(predecessor_no) = unverifiable_step {
        json!({
            "verdict": "unverifiable",
            "reason": format!("no output schema for step {predecessor_no}"),
            "next": {"tool": "host_tool_call"},
        })
    } else {
        json!({"verdict": "pass"})
    };
    StepsVerdict { overall, passes }
}

/// PRD-mcphost-tool-test-truth P1 requirement 1 (AC6): `host.tool_publish`'s
/// own entry point into [`steps_verdict`] -- parses `spec` (an
/// already-published chain's spec always parses; `validate` already
/// rejected anything else at publish time, so the fallback below exists
/// only so an unpublished/malformed spec never panics) and discards the
/// per-field `passes` set, which only [`dry_run_report`] needs.
pub(crate) async fn verdict(
    db: &crate::db::Db,
    kinds: &KindRegistry,
    tenant_id: i64,
    spec: &Value,
) -> Value {
    let Ok(steps) = parse_steps(spec) else {
        return json!({"verdict": "unverifiable", "reason": "spec does not parse"});
    };
    let mut overall = steps_verdict(Some(db), Some(kinds), tenant_id, &steps, true).await.overall;
    // PRD-mcphost-chain-publish-time-path-check requirement 2 (AC3): a
    // predecessor nothing is known about yet is settled by running it.
    if overall["verdict"] == json!("unverifiable") {
        overall["next"] = json!({"tool": "host.tool_test"});
    }
    overall
}

/// PRD-mcphost-chain-prev-contract P1 requirement 6: per step after the
/// first, the fields it may read from its predecessor under
/// `$.prev.result` -- the predecessor's declared `outputs` when it has any
/// (`prev_fields_source: "declared"`); otherwise `prev_fields` is `null`
/// until a `host.tool_test` dry run reports it from the actual result.
pub(crate) async fn prev_fields(
    db: &crate::db::Db,
    kinds: &KindRegistry,
    tenant_id: i64,
    spec: &Value,
) -> Value {
    let Ok(steps) = parse_steps(spec) else {
        return json!([]);
    };
    let mut out = Vec::new();
    for (i, step) in steps.iter().enumerate().skip(1) {
        let basis = path_basis(db, kinds, tenant_id, &steps[i - 1].tool, true).await;
        let entry = match basis {
            None => json!({"step": i + 1, "tool": step.tool, "prev_fields": null,
                   "hint": "predecessor declares no outputs and has no completed run; host.tool_test reports prev_fields from its dry-run result",
                   "next": {"tool": "host.tool_test"}}),
            Some(b) => {
                // Observed keys are reported sorted (a set), declared keys in
                // declaration order.
                let mut e = json!({"step": i + 1, "tool": step.tool, "prev_fields": b.names, "prev_fields_source": b.source});
                if let Some(n) = b.runs_sampled {
                    e["runs_sampled"] = json!(n);
                    if b.source == "declared" {
                        e["observed_drift"] = json!(b.drift);
                    }
                }
                e
            }
        };
        out.push(entry);
    }
    Value::Array(out)
}

/// PRD-mcphost-chain-run-lineage requirement 1: one `$.input.<name>`
/// reference, first-appearance order -- `name` is the schema
/// property/required entry it contributes; `step_no`/`tool` (1-based) name
/// the first step that references it, for [`ChainKind::call`]'s own
/// call-time pre-check (requirement 3) to point at when that name turns out
/// to be missing from the call's actual args.
struct InputRef {
    name: String,
    step_no: usize,
    tool: String,
}

/// Requirement 1: the argument name a `$.input.<name>[...]` path
/// contributes -- the first segment after `input.`, up to the next `.` or
/// `[`. Deliberately NOT `super::Path::parse` (whose parsed segments are
/// private to this crate's mapping-resolution code, and whose stricter
/// grammar checking belongs at call time, not schema-derivation time,
/// exactly like `resolve_args` below already treats an unparseable path as
/// "doesn't resolve," not a publish-time error) -- this only needs the
/// first segment's name, on a raw string that may not even be non-`$.`
/// yet (a step's `args` value that doesn't start with `"$."` is a literal,
/// never reaches here).
fn input_arg_name(raw: &str) -> Option<String> {
    let rest = raw.strip_prefix("$.input.")?;
    let end = rest.find(['.', '[']).unwrap_or(rest.len());
    (!rest[..end].is_empty()).then(|| rest[..end].to_string())
}

/// Requirement 1: every step's `args`, in step order, contributing each
/// `$.input.<name>` reference it makes -- deduplicated by name (first
/// appearance wins), so `daily_pipeline`'s `region` used by both step 1 and
/// step 3 appears once, attributed to step 1.
fn collect_input_refs(steps: &[ParsedStep]) -> Vec<InputRef> {
    let mut seen = std::collections::HashSet::new();
    let mut refs = Vec::new();
    for (i, step) in steps.iter().enumerate() {
        for s in step.args.values().filter_map(Value::as_str).chain(step.for_each.as_deref()) {
            if let Some(name) = input_arg_name(s)
                && seen.insert(name.clone())
            {
                refs.push(InputRef {
                    name,
                    step_no: i + 1,
                    tool: step.tool.clone(),
                });
            }
        }
    }
    refs
}

/// Requirement 1: `describe()`'s `input_schema` -- exactly `{"type":
/// "object"}` for a chain with no `$.input.*` paths at all (unchanged from
/// before this PRD), else an object schema naming every referenced input,
/// all required, typed `{}` (permissive -- a step's own mapping doesn't pin
/// a type, only that the value must be present).
fn chain_input_schema(steps: &[ParsedStep]) -> Value {
    let refs = collect_input_refs(steps);
    if refs.is_empty() {
        return json!({"type": "object"});
    }
    let properties: Map<String, Value> = refs.iter().map(|r| (r.name.clone(), json!({}))).collect();
    let required: Vec<&str> = refs.iter().map(|r| r.name.as_str()).collect();
    json!({
        "type": "object",
        "properties": Value::Object(properties),
        "required": required,
    })
}

/// One parsed step: the target tool's local name, its own literal/path
/// argument mapping (kept as raw `Value`s -- resolved fresh per call, and
/// per dry-run report), and whether a failure here stops the chain
/// (`"stop"`, the default) or lets the remaining steps run (`"continue"`,
/// requirement 3 / AC7).
struct ParsedStep {
    tool: String,
    args: Map<String, Value>,
    /// PRD-mcphost-jq-kind R5: `"args": "$.prev"` (a string, not an object)
    /// passes the previous step's result whole as this step's arguments.
    prev_whole: bool,
    continue_on_error: bool,
    /// PRD-mcphost-chain-fanout-and-join R1: the `$.<path>` of the array this
    /// step runs once per element of.
    for_each: Option<String>,
    /// R1: this step's own in-flight cap (clamped to the plan's at call time).
    concurrency: Option<usize>,
}

fn invalid_spec(message: impl Into<String>) -> KindError {
    KindError::InvalidSpec(message.into())
}

/// PRD-mcphost-retry-policy-table-generated R5: a step's `retry.on` names
/// only codes `RETRY_POLICY` marks retryable -- resolved through
/// [`crate::retry_policy::policy_for`], never a list of its own. A refusal
/// carries `data.accepted`, the table's retryable codes. (The host does not
/// retry a step itself; this is the publish-time contract.)
fn validate_retry_on(i: usize, retry: &Value) -> Result<(), KindError> {
    let on = retry
        .get("on")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid_spec(format!("spec.steps[{i}].retry.on: must be an array of error codes")))?;
    let accepted = crate::retry_policy::retryable_codes();
    let refused: Vec<&str> = on
        .iter()
        .filter_map(|c| match c.as_str() {
            Some(code) if crate::retry_policy::is_retryable_code(code) => None,
            Some(code) => Some(code),
            None => Some("<non-string>"),
        })
        .collect();
    if refused.is_empty() {
        return Ok(());
    }
    Err(KindError::structured_with(
        crate::errors::INVALID_SPEC,
        format!(
            "spec.steps[{i}].retry.on: {} not retryable; accepted codes are listed in data.accepted",
            refused.join(", ")
        ),
        json!({"step": i + 1, "field": format!("steps[{i}].retry.on"), "refused": refused, "accepted": accepted}),
    ))
}

/// Requirement 3: parses and shape-checks `spec.steps`. Does not resolve or
/// parse any `"$."` path inside a step's `args` -- a malformed mapping path
/// is reported at call time (requirement 4), naming the step and path, not
/// at publish time, since publish has no `context` to resolve against yet
/// either way.
fn parse_steps(spec: &Value) -> Result<Vec<ParsedStep>, KindError> {
    let obj = spec
        .as_object()
        .ok_or_else(|| invalid_spec("spec: must be a JSON object"))?;
    let steps_raw = obj
        .get(CONTEXT_KEYS.steps)
        .ok_or_else(|| invalid_spec("spec.steps: is required"))?
        .as_array()
        .ok_or_else(|| invalid_spec("spec.steps: must be an array"))?;
    if steps_raw.is_empty() {
        return Err(invalid_spec("spec.steps: must have at least one step"));
    }

    let mut steps = Vec::with_capacity(steps_raw.len());
    for (i, raw) in steps_raw.iter().enumerate() {
        let step_obj = raw
            .as_object()
            .ok_or_else(|| invalid_spec(format!("spec.steps[{i}]: must be a JSON object")))?;
        let tool = step_obj
            .get("tool")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid_spec(format!("spec.steps[{i}].tool: is required")))?
            .to_string();
        let mut prev_whole = false;
        let args = match step_obj.get("args") {
            None => Map::new(),
            Some(Value::Object(m)) => m.clone(),
            Some(Value::String(s)) if s == "$.prev" && i > 0 => {
                prev_whole = true;
                Map::new()
            }
            Some(_) => {
                return Err(invalid_spec(format!(
                    "spec.steps[{i}].args: must be a JSON object, or the string \"$.prev\" on a step after the first"
                )));
            }
        };
        let continue_on_error = match step_obj.get("on_error") {
            None => false,
            Some(Value::String(s)) if s == "stop" => false,
            Some(Value::String(s)) if s == "continue" => true,
            Some(_) => {
                return Err(invalid_spec(format!(
                    "spec.steps[{i}].on_error: must be \"stop\" or \"continue\""
                )));
            }
        };
        if let Some(retry) = step_obj.get("retry") {
            validate_retry_on(i, retry)?;
        }
        let for_each = match step_obj.get("for_each") {
            None => None,
            Some(Value::String(s)) if Path::parse(s).is_ok() => Some(s.clone()),
            Some(_) => {
                return Err(invalid_spec(format!(
                    "spec.steps[{i}].for_each: must be a mapping path string such as \"$.prev.result.items\""
                )));
            }
        };
        let concurrency = match step_obj.get("concurrency") {
            None => None,
            Some(v) if for_each.is_some() => match v.as_u64() {
                Some(n) if n >= 1 => Some(n as usize),
                _ => {
                    return Err(invalid_spec(format!(
                        "spec.steps[{i}].concurrency: must be an integer of at least 1"
                    )));
                }
            },
            Some(_) => {
                return Err(invalid_spec(format!("spec.steps[{i}].concurrency: only a for_each step may set it")));
            }
        };
        if for_each.is_none()
            && let Some(raw) = args.values().find_map(|v| v.as_str().filter(|s| is_item_ref(s)))
        {
            return Err(KindError::structured_with(
                codes::COMPOSE_ITEM_OUTSIDE_FOR_EACH,
                format!("spec.steps[{i}]: '{raw}' is only available inside a step that declares for_each"),
                json!({"step": i + 1, "path": raw}),
            ));
        }
        if prev_whole && for_each.is_some() {
            return Err(invalid_spec(format!(
                "spec.steps[{i}].args: \"$.prev\" cannot be combined with for_each"
            )));
        }
        steps.push(ParsedStep {
            tool,
            args,
            prev_whole,
            continue_on_error,
            for_each,
            concurrency,
        });
    }
    Ok(steps)
}

/// Requirement 4: resolves one step's raw `args` map against `context`
/// (`{"input", "prev", "steps"}`) into the concrete arguments that step is
/// called with. A top-level string value starting with `"$."` is a path
/// (parsed with the exact grammar `outputs` paths use); anything else --
/// including a string that doesn't start with `"$."` -- is a literal,
/// copied as-is. Returns the offending path (not yet wrapped in a
/// `KindError`, so the caller can name the step number too) the moment one
/// resolves to nothing.
fn resolve_args(raw: &Map<String, Value>, context: &Value) -> Result<Value, String> {
    let mut out = Map::with_capacity(raw.len());
    for (k, v) in raw {
        let resolved = match v {
            Value::String(s) if s.starts_with("$.") => {
                let path = Path::parse(s).map_err(|_| s.clone())?;
                path.resolve(context).cloned().ok_or_else(|| s.clone())?
            }
            other => other.clone(),
        };
        out.insert(k.clone(), resolved);
    }
    Ok(Value::Object(out))
}

/// A [`KindError`] as `(code, message, data)`.
fn kind_error_parts(e: &KindError) -> (&'static str, String, Value) {
    match e {
        KindError::Structured { code, message, data } => (*code, message.clone(), data.clone()),
        KindError::InvalidSpec(m) => ("invalid_spec", m.clone(), Value::Null),
        KindError::InvalidArgs(m) => ("args_invalid", m.clone(), Value::Null),
        KindError::Exec(m) => ("exec", m.clone(), Value::Null),
    }
}

/// What one `for_each` step produced: the joined result and the lowest-index
/// item failure, if any.
struct FanoutOutcome {
    join: Value,
    first_failure: Option<(usize, String, String)>,
    concurrency: usize,
}

/// One item's end state. `Skipped` is an item never started because an
/// earlier item failed under `on_error: stop`.
enum ItemEnd {
    Done(Value),
    Failed(String, String),
    Skipped,
}

/// PRD-mcphost-chain-fanout-and-join R2/R3/R4/R5/R6: resolves `path` to an
/// array, refuses past the plan ceiling before anything runs, then runs
/// `step` once per element through [`compose_call_ref`] under a semaphore of
/// `min(step.concurrency, plan.concurrent_calls_per_tenant)` and joins the
/// results in input order.
async fn run_fanout(
    ctx: &CallCtx,
    step: &ParsedStep,
    step_no: usize,
    context: &Value,
    path: &str,
) -> Result<FanoutOutcome, KindError> {
    let base_data = |extra: Value| {
        let mut data = Map::new();
        data.insert("path".to_string(), json!(path));
        data.insert("failed_step".to_string(), json!(step_no));
        data.insert("step_tool".to_string(), json!(step.tool));
        if let Value::Object(m) = extra {
            data.extend(m);
        }
        Value::Object(data)
    };
    let items = match Path::parse(path).ok().and_then(|p| p.resolve(context).cloned()) {
        Some(Value::Array(items)) => items,
        Some(other) => {
            return Err(KindError::structured_with(
                codes::COMPOSE_MAPPING_TYPE,
                format!(
                    "step {step_no} ('{}'): for_each path '{path}' is {}, expected an array",
                    step.tool,
                    json_type(&other)
                ),
                base_data(json!({"expected": "array", "actual": json_type(&other)})),
            ));
        }
        None => {
            return Err(KindError::structured_with(
                codes::COMPOSE_MAPPING_MISSING,
                format!("step {step_no} ('{}'): for_each path '{path}' resolved to nothing", step.tool),
                base_data(json!({})),
            ));
        }
    };
    if items.len() > ctx.chain_fanout_max {
        return Err(KindError::structured_with(
            codes::COMPOSE_FANOUT_EXCEEDED,
            format!(
                "step {step_no} ('{}'): {} items exceed this plan's chain_fanout_max of {}",
                step.tool,
                items.len(),
                ctx.chain_fanout_max
            ),
            json!({"limit": ctx.chain_fanout_max, "actual": items.len()}),
        ));
    }

    let width = step
        .concurrency
        .unwrap_or(usize::MAX)
        .min(ctx.concurrent_calls_per_tenant)
        .clamp(1, tokio::sync::Semaphore::MAX_PERMITS);
    let permits = tokio::sync::Semaphore::new(width);
    let stop = std::sync::atomic::AtomicBool::new(false);
    let ends = futures_util::future::join_all(items.iter().enumerate().map(|(index, item)| {
        let (permits, stop) = (&permits, &stop);
        async move {
            let Ok(_permit) = permits.acquire().await else {
                return ItemEnd::Skipped;
            };
            if stop.load(std::sync::atomic::Ordering::SeqCst) {
                return ItemEnd::Skipped;
            }
            // Items share the chain's deadline: one that cannot start in
            // time is failed, not dropped.
            if std::time::Instant::now() >= ctx.deadline {
                return ItemEnd::Failed("deadline_exceeded".to_string(), "the chain's deadline passed before this item could start".to_string());
            }
            let args = match resolve_args(&step.args, &item_context(context, item, index)) {
                Ok(args) => args,
                Err(bad) => {
                    return ItemEnd::Failed(
                        codes::COMPOSE_MAPPING_MISSING.to_string(),
                        format!("mapping path '{bad}' resolved to nothing"),
                    );
                }
            };
            match compose_call_ref(
                ctx,
                ctx.tenant_id,
                &step.tool,
                args,
                None,
                Some(step_no as i64),
                Some(index as i64),
                context.get(CONTEXT_KEYS.prev).and_then(|p| p.get(CONTEXT_KEYS.result)).cloned(),
            )
            .await
            {
                Ok(result) => ItemEnd::Done(result),
                Err(e) => {
                    if !step.continue_on_error {
                        stop.store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                    let (code, message, _) = kind_error_parts(&e);
                    ItemEnd::Failed(code.to_string(), message)
                }
            }
        }
    }))
    .await;

    let mut joined = Vec::with_capacity(ends.len());
    let mut failed = Vec::new();
    let mut first_failure = None;
    let mut ok = 0usize;
    for (index, end) in ends.into_iter().enumerate() {
        match end {
            ItemEnd::Done(result) => {
                ok += 1;
                joined.push(result);
            }
            ItemEnd::Failed(code, message) => {
                first_failure.get_or_insert((index, code.clone(), message.clone()));
                failed.push(json!({"index": index, "error_code": code, "message": message}));
                joined.push(Value::Null);
            }
            ItemEnd::Skipped => joined.push(Value::Null),
        }
    }
    Ok(FanoutOutcome {
        join: json!({"items": joined, "failed": failed, "ok": ok, "failed_count": failed.len()}),
        first_failure,
        concurrency: width,
    })
}

/// Requirement 3 / AC8: `host.tool_test`'s dry-run report for a chain --
/// each step's resolved arguments, without dispatching an ordinary
/// tenant-tool step. A mapping that reaches into `$.prev`/`$.steps[i]`
/// can't be resolved against a step that was never dispatched, so it's
/// reported as `{"unresolved_path": "$..."}` instead of a value -- UNLESS
/// [`verdict`]'s own static check (PRD-mcphost-tool-test-truth AC2) has
/// already confirmed that exact mapping resolves against its predecessor's
/// declared outputs, in which case it's left out of `resolved_args`
/// entirely (still no real value to report without dispatching, but no
/// longer claimed broken either); `$.input.*` and literals resolve exactly
/// as a real call would.
///
/// PRD-mcphost-tool-test-truth P0 requirement 1/2 (AC1): a `will_fail`
/// verdict short-circuits before this function's own per-step loop ever
/// runs (even its host-step dispatch, AC5 below) -- see this function's
/// own early return.
///
/// PRD-mcphost-chain-host-steps requirement 4/AC4: each step also carries
/// `resolved` (`"host"` for an allowlisted `host.*` verb, `"tenant"` for a
/// sibling tool -- a step here was already resolved once, at publish time
/// (`resolve_steps`), so this is a pure syntactic re-derivation, no DB
/// lookup needed).
///
/// PRD-mcphost-dry-run-side-effects requirement 1/2 (AC5): a step naming an
/// allowlisted `host.*` verb ([`HOST_STEPS_ALLOWED`]) actually dispatches,
/// through `ctx.host_dispatch`, so its write lands inside the `DryRunCtx`
/// savepoint `host.tool_test` already opened around this whole call and is
/// reported into the envelope's `dry_run.writes` by the same bridge a
/// sandboxed tool's own `mcphost.table`/`mcphost.state` call would go
/// through. Its real result then feeds `$.prev`/`$.steps[i]` for any later
/// step exactly like a real run's would. `ctx.host_dispatch` is `None`
/// outside `host.tool_test` (e.g. `host.spec_test`'s own dry run) -- a host
/// step there still carries `"resolved": "host"`/`side_effects: true` but
/// is not dispatched, same as an unresolved-args host step.
///
/// Named `chain_dry_run` (not `dry_run`) so it never collides with the
/// result envelope's own `dry_run: {writes, delivered, rolled_back}` key
/// `host.tool_test`/`host.tool_run(test: true)` insert via `.entry()`.
///
/// PRD-mcphost-chain-run-lineage requirement 2 (AC10): also reports
/// `inputs_required` -- the same list [`chain_input_schema`] derives into
/// `input_schema.required` -- alongside the existing per-step trace, so a
/// caller dry-running a chain (with or without a complete `call_args`)
/// learns what it must pass without needing a second `host.tool_spec` read.
/// PRD-mcphost-chain-prev-contract P0 requirement 3: executes tenant tool
/// `tool` once, under the dry run's own `ctx` (so its writes land in the
/// open `DryRunCtx` savepoint and roll back), to learn the shape of the
/// result a later step's `$.prev.result.<field>` reads from. `Err` carries
/// why the predecessor could not be dry-run (no such tool, nested chain,
/// upstream down, deadline) -- the only case a mapping into it is
/// `unverifiable`.
async fn dry_run_predecessor(ctx: &CallCtx, tool: &str, args: Value) -> Result<Value, String> {
    let (Some(db), Some(kinds)) = (ctx.compose_db.as_ref(), ctx.compose_kinds.as_ref()) else {
        return Err("no tool lookup available in this call context".to_string());
    };
    let row = db
        .get_tool(ctx.tenant_id, tool.to_string())
        .await
        .map_err(|e| format!("tool lookup failed: {e}"))?
        .ok_or_else(|| format!("no such tool: {tool}"))?;
    let kind = kinds
        .get(&row.kind)
        .ok_or_else(|| format!("unregistered kind '{}'", row.kind))?;
    if kind.name() == "chain" {
        return Err("a nested chain has no single result shape to dry-run".to_string());
    }
    let remaining = ctx.deadline.saturating_duration_since(std::time::Instant::now());
    let value = tokio::time::timeout(remaining, kind.call(&row.spec, args, ctx))
        .await
        .map_err(|_| "dry run deadline reached".to_string())?
        .map_err(|e| e.to_string())?;
    // `http` in test mode wraps what a real call would return in
    // `{request, response, schema}`; the real result is `response`.
    Ok(match (kind.name(), value) {
        ("http", Value::Object(mut m)) if m.contains_key("response") => m.remove("response").unwrap_or(Value::Null),
        (_, v) => v,
    })
}

/// The mapping failure evidence for a `$.prev`/`$.steps[i]` path that did
/// not resolve against `base` (the predecessor's actual result):
/// `available` is its top-level keys, `did_you_mean` the corrected path
/// when a key is a plausible match for the missing segment.
fn actual_result_evidence(r: &StepRef, base: &Value) -> Map<String, Value> {
    let available: Vec<String> = match base {
        Value::Object(m) => m.keys().cloned().collect(),
        _ => Vec::new(),
    };
    let mut out = Map::new();
    let suggestion = match (r.name(0), r.name(1)) {
        // Missing hop: `$.prev.<seg>` where `<seg>` is a field of the result.
        (Some(seg), _) if !r.accepted_keys().contains(seg) => {
            closest_name(seg, &available).map(|near| r.rebuilt(&[CONTEXT_KEYS.result], Some((0, near))))
        }
        (Some(hop), Some(field)) if hop == CONTEXT_KEYS.result && !available.iter().any(|k| k == field) => {
            closest_name(field, &available).map(|near| r.rebuilt(&[], Some((1, near))))
        }
        _ => None,
    };
    if let Some(s) = suggestion {
        out.insert("did_you_mean".to_string(), json!(s));
    }
    out.insert("available".to_string(), json!(available));
    out
}

tokio::task_local! {
    /// PRD-mcphost-chain-dry-run: the `fixtures` argument of the enclosing
    /// `host.tool_test`, keyed by the caller's raw step key (1-based number
    /// or step tool name). Task-local (not a `CallCtx` field) because it is
    /// per-request and read only by [`dry_run_report`].
    static DRY_RUN_FIXTURES: Map<String, Value>;
}

/// Runs `fut` with `fixtures` visible to the chain dry run inside it.
pub async fn with_fixtures<F: std::future::Future>(fixtures: Map<String, Value>, fut: F) -> F::Output {
    DRY_RUN_FIXTURES.scope(fixtures, fut).await
}

/// Resolves the caller's fixture keys (1-based step number, or the tool name
/// of the first step using it) to step numbers.
fn resolve_fixture_steps(
    steps: &[ParsedStep],
    raw: &Map<String, Value>,
) -> Result<std::collections::BTreeMap<usize, Value>, KindError> {
    let mut out = std::collections::BTreeMap::new();
    for (key, value) in raw {
        let step_no = if let Ok(n) = key.parse::<usize>() {
            if n == 0 || n > steps.len() {
                return Err(KindError::structured_with(
                    FIXTURE_STEP_UNKNOWN,
                    format!("fixture key '{key}': the chain has {} steps", steps.len()),
                    json!({"key": key, "steps": steps.len()}),
                ));
            }
            n
        } else {
            let matches: Vec<usize> = steps
                .iter()
                .enumerate()
                .filter(|(_, s)| s.tool == *key)
                .map(|(i, _)| i + 1)
                .collect();
            match matches.as_slice() {
                [] => {
                    return Err(KindError::structured_with(
                        FIXTURE_STEP_UNKNOWN,
                        format!("fixture key '{key}': no step uses a tool with that name"),
                        json!({"key": key, "steps": steps.len()}),
                    ));
                }
                [n] => *n,
                _ => {
                    return Err(KindError::structured_with(
                        FIXTURE_STEP_AMBIGUOUS,
                        format!("fixture key '{key}': steps {matches:?} all use that tool; use the step number"),
                        json!({"key": key, "steps": matches}),
                    ));
                }
            }
        };
        out.insert(step_no, value.clone());
    }
    Ok(out)
}

async fn dry_run_report(steps: &[ParsedStep], call_args: &Value, ctx: &CallCtx) -> Result<Value, KindError> {
    let inputs_required: Vec<String> = collect_input_refs(steps).into_iter().map(|r| r.name).collect();
    let sv = steps_verdict(
        ctx.compose_db.as_ref(),
        ctx.compose_kinds.as_ref(),
        ctx.tenant_id,
        steps,
        false,
    )
    .await;
    let fixtures = match DRY_RUN_FIXTURES.try_with(|f| f.clone()) {
        Ok(raw) => resolve_fixture_steps(steps, &raw)?,
        Err(_) => Default::default(),
    };
    // R4: a fixture for a step whose tool declares `outputs` must carry at
    // least one of them -- refused with both key sets, before anything runs.
    if let (Some(db), Some(kinds)) = (ctx.compose_db.as_ref(), ctx.compose_kinds.as_ref()) {
        for (step_no, fixture) in &fixtures {
            let tool = &steps[step_no - 1].tool;
            let expected = declared_output_names(db, kinds, ctx.tenant_id, tool).await.unwrap_or_default();
            if expected.is_empty() {
                continue;
            }
            let given: Vec<String> = fixture.as_object().map(|m| m.keys().cloned().collect()).unwrap_or_default();
            if !expected.iter().any(|k| given.contains(k)) {
                return Err(KindError::structured_with(
                    FIXTURE_SHAPE_MISMATCH,
                    format!(
                        "fixture for step {step_no} ('{tool}') carries none of the declared outputs {expected:?} (given keys {given:?})"
                    ),
                    json!({"step": step_no, "expected": expected, "given": given}),
                ));
            }
        }
    }
    if sv.overall["verdict"] == json!("will_fail") {
        return Ok(json!({
            "chain_dry_run": true,
            (CONTEXT_KEYS.steps): [],
            "inputs_required": inputs_required,
            "verdict": "will_fail",
            "failures": sv.overall["failures"].clone(),
            "evidence": sv.overall["failures"].clone(),
        }));
    }

    // Steps whose actual dry-run result some later mapping reads and the
    // static check could not already settle from declared `outputs`.
    let mut needed: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for (i, step) in steps.iter().enumerate() {
        if step.prev_whole {
            needed.insert(i);
        }
        for raw in step.args.values().filter_map(Value::as_str).chain(step.for_each.as_deref()) {
            if let Some(r) = StepRef::parse(raw, i + 1)
                && !r.tokens.is_empty()
                && !sv.passes.contains(&(i + 1, raw.to_string()))
            {
                needed.insert(r.predecessor_no);
            }
        }
    }

    let mut prev: Option<Value> = None;
    let mut step_results: Vec<Value> = Vec::with_capacity(steps.len());
    let mut report: Vec<Value> = Vec::with_capacity(steps.len());
    let mut actual_failures: Vec<Value> = Vec::new();
    // step_no -> (tool, why it could not be dry-run)
    let mut not_runnable: std::collections::BTreeMap<usize, (String, String)> = std::collections::BTreeMap::new();
    let mut any_unresolved: Option<(usize, String)> = None;
    // With fixtures in play every tenant step is dry-run (not just those a
    // later mapping reads), so each carries its `result`.
    let run_all = !fixtures.is_empty();
    for (i, step) in steps.iter().enumerate() {
        let step_no = i + 1;
        let context = step_context(call_args, prev.as_ref(), &step_results);
        let mut resolved_map = Map::with_capacity(step.args.len());
        let mut unresolved = false;
        // PRD-mcphost-chain-fanout-and-join R7: a `for_each` step is reported
        // once, its args resolved for the first item only; no item executes.
        let mut fanout: Option<Value> = None;
        let context = match step.for_each.as_deref() {
            Some(path) => {
                let found = Path::parse(path).ok().and_then(|p| p.resolve(&context).cloned());
                if !matches!(found, Some(Value::Array(_))) && !sv.passes.contains(&(step_no, path.to_string())) {
                    unresolved = true;
                    any_unresolved.get_or_insert((step_no, path.to_string()));
                }
                let first = found.as_ref().and_then(Value::as_array).and_then(|a| a.first()).cloned();
                fanout = Some(json!({
                    "path": path,
                    "concurrency": step.concurrency.unwrap_or(usize::MAX).min(ctx.concurrent_calls_per_tenant),
                    "limit": ctx.chain_fanout_max,
                }));
                item_context(&context, &first.unwrap_or(Value::Null), 0)
            }
            None => context,
        };
        for (k, v) in &step.args {
            let value = match v {
                Value::String(s) if s.starts_with("$.") => {
                    match Path::parse(s).ok().and_then(|p| p.resolve(&context).cloned()) {
                        Some(v) => v,
                        None if sv.passes.contains(&(step_no, s.clone())) => Value::Null,
                        None => {
                            unresolved = true;
                            any_unresolved.get_or_insert((step_no, s.clone()));
                            // The predecessor ran and its actual result
                            // lacks this path: a proven failure, with the
                            // fields it does carry.
                            if let Some(r) = StepRef::parse(s, step_no)
                                && let Some(base) = r.base_result(&context)
                            {
                                let mut entry = Map::new();
                                entry.insert("step".to_string(), json!(step_no));
                                entry.insert("path".to_string(), json!(s));
                                entry.extend(actual_result_evidence(&r, base));
                                actual_failures.push(Value::Object(entry));
                            }
                            json!({"unresolved_path": s})
                        }
                    }
                }
                other => other.clone(),
            };
            resolved_map.insert(k.clone(), value);
        }
        let resolved = if step.prev_whole {
            prev.clone().unwrap_or_else(|| {
                unresolved = true;
                any_unresolved.get_or_insert((step_no, "$.prev".to_string()));
                json!({"unresolved_path": "$.prev"})
            })
        } else {
            Value::Object(resolved_map)
        };
        let mut prev_fields = None;
        if i > 0
            && let Some(Value::Object(m)) = prev.as_ref()
        {
            prev_fields = Some(m.keys().cloned().collect::<Vec<_>>());
        }
        let is_host = HOST_STEPS_ALLOWED.contains(&step.tool.as_str());
        // PRD-mcphost-chain-dry-run R2: a fixtured step executes nothing --
        // the recorded value stands in for its result.
        if let Some(fixture) = fixtures.get(&step_no) {
            let mut entry = json!({
                "step": step_no,
                "tool": step.tool,
                "source": "fixture",
                "resolved_args": resolved,
                "resolved": if is_host { "host" } else { "tenant" },
                "result": fixture,
            });
            insert_prev_fields(&mut entry, &prev_fields);
            report.push(entry);
            step_results.push(fixture.clone());
            prev = Some(fixture.clone());
            continue;
        }
        if let Some(fanout) = fanout {
            let mut entry = json!({
                "step": step_no,
                "tool": step.tool,
                "resolved_args": resolved,
                "resolved": if is_host { "host" } else { "tenant" },
                "fanout": fanout,
                "source": "skipped",
                "reason": "a for_each step executes no item in a dry run",
            });
            insert_prev_fields(&mut entry, &prev_fields);
            report.push(entry);
            step_results.push(Value::Null);
            prev = None;
            continue;
        }
        if is_host
            && !unresolved
            && let Some(host) = ctx.host_dispatch.as_ref()
        {
            match host.call(&step.tool, resolved.clone()).await {
                Ok(result) => {
                    let mut entry = json!({
                        "step": step_no,
                        "tool": step.tool,
                        "resolved_args": resolved,
                        "source": "dry_run",
                        "resolved": "host",
                        "side_effects": true,
                        "dispatched": true,
                        (CONTEXT_KEYS.result): result,
                    });
                    insert_prev_fields(&mut entry, &prev_fields);
                    report.push(entry);
                    step_results.push(result.clone());
                    prev = Some(result);
                    continue;
                }
                Err(e) => {
                    let mut entry = json!({
                        "step": step_no,
                        "tool": step.tool,
                        "resolved_args": resolved,
                        "source": "dry_run",
                        "resolved": "host",
                        "side_effects": true,
                        "dispatched": true,
                        "error": e.to_string(),
                    });
                    insert_prev_fields(&mut entry, &prev_fields);
                    report.push(entry);
                    not_runnable.insert(step_no, (step.tool.clone(), e.to_string()));
                    step_results.push(Value::Null);
                    prev = None;
                    continue;
                }
            }
        }
        let mut entry = json!({
            "step": step_no,
            "tool": step.tool,
            "resolved_args": resolved,
            "resolved": if is_host { "host" } else { "tenant" },
        });
        if is_host
            && let Value::Object(map) = &mut entry
        {
            map.insert("side_effects".to_string(), json!(true));
        }
        insert_prev_fields(&mut entry, &prev_fields);
        // PRD-mcphost-chain-prev-contract P0 requirement 3: a tenant step a
        // later mapping reads is dry-run once so that mapping can be
        // resolved against its actual result.
        let mut skip_reason = "no later step reads its result".to_string();
        if !is_host && (run_all || needed.contains(&step_no)) {
            let outcome = if unresolved {
                Err("its own arguments did not resolve".to_string())
            } else {
                dry_run_predecessor(ctx, &step.tool, resolved.clone()).await
            };
            match outcome {
                Ok(result) => {
                    if let Value::Object(map) = &mut entry {
                        map.insert("source".to_string(), json!("dry_run"));
                        map.insert("dispatched".to_string(), json!(true));
                        map.insert(CONTEXT_KEYS.result.to_string(), result.clone());
                    }
                    report.push(entry);
                    step_results.push(result.clone());
                    prev = Some(result);
                    continue;
                }
                Err(why) => {
                    if let Value::Object(map) = &mut entry {
                        map.insert("error".to_string(), json!(why));
                    }
                    skip_reason = why.clone();
                    not_runnable.insert(step_no, (step.tool.clone(), why));
                }
            }
        } else if is_host {
            skip_reason = "a host step is dispatched only when its arguments resolve".to_string();
        }
        if let Value::Object(map) = &mut entry {
            map.insert("source".to_string(), json!("skipped"));
            map.insert("reason".to_string(), json!(skip_reason));
        }
        report.push(entry);
        step_results.push(Value::Null);
        prev = None;
    }
    let count = |source: &str| report.iter().filter(|e| e["source"] == json!(source)).count();
    let (executed, fixtured, skipped) = (count("dry_run"), count("fixture"), count("skipped"));
    let mut out = json!({
        "chain_dry_run": true,
        (CONTEXT_KEYS.steps): report,
        "inputs_required": inputs_required,
        "executed": executed,
        "fixtured": fixtured,
        "skipped": skipped,
    });
    // Verdict: a proven mismatch against an actual result is `will_fail`;
    // `unverifiable` only when a predecessor a mapping reads could not be
    // dry-run (or a path is still unresolved for another reason -- a
    // dry run never reports `pass` with an `unresolved_path` in it);
    // otherwise the static verdict, upgraded to `pass` when every mapping
    // it could not settle was resolved against an actual result.
    let unreadable = needed.iter().find_map(|n| not_runnable.get(n).map(|w| (*n, w.clone())));
    let (verdict, reason): (&str, Option<String>) = if !actual_failures.is_empty() {
        ("will_fail", None)
    } else if let Some((n, (tool, why))) = unreadable {
        (
            "unverifiable",
            Some(format!("no output schema for step {n} ('{tool}'), and it could not be dry-run: {why}")),
        )
    } else if let Some((n, path)) = any_unresolved {
        (
            "unverifiable",
            Some(format!("step {n}: mapping path '{path}' did not resolve in the dry run")),
        )
    } else {
        ("pass", None)
    };
    if let Value::Object(map) = &mut out {
        map.insert("verdict".to_string(), json!(verdict));
        if verdict == "will_fail" {
            map.insert("failures".to_string(), json!(actual_failures));
            map.insert("evidence".to_string(), json!(actual_failures));
        }
        if let Some(reason) = reason {
            map.insert("reason".to_string(), json!(reason));
            map.insert("next".to_string(), json!({"tool": "host_tool_call"}));
        }
    }
    Ok(out)
}

fn insert_prev_fields(entry: &mut Value, prev_fields: &Option<Vec<String>>) {
    if let (Value::Object(map), Some(fields)) = (entry, prev_fields) {
        map.insert("prev_fields".to_string(), json!(fields));
    }
}

fn conformance_good_spec() -> Value {
    let msg_path = format!("$.{}.msg", CONTEXT_KEYS.input);
    json!({ (CONTEXT_KEYS.steps): [{"tool": "step_one", "args": {"msg": msg_path}}] })
}

/// Reads `$.input.msg` from a call that supplies no `msg`.
fn conformance_erroring_spec() -> Value {
    conformance_good_spec()
}

static CONTRACT: KindContract = KindContract {
    error_families: &[COMPOSE_FAMILY, TOOL_STATE_FAMILY],
    budgets: &[],
    source_position: SourcePosition::Field("failed_step"),
    bridges: &[],
    secret_redaction: false,
    fixtures: ConformanceFixtures {
        good_spec: conformance_good_spec,
        good_args: || json!({"msg": "hi"}),
        bad_args: || json!("not an object"),
        erroring_spec: Some(conformance_erroring_spec),
        slow_spec: None,
        secret_spec: None,
        skips: &[
            CaseSkip {
                case: CASE_CALL,
                reason: "a chain call dispatches each step through the tenant's tool table; the suite's CallCtx wires no composition",
            },
            CaseSkip {
                case: CASE_BUDGET_EXCEEDED,
                reason: "chain declares no budget dimension of its own; its children are budgeted by compose_call",
            },
        ],
    },
};

#[async_trait::async_trait]
impl Kind for ChainKind {
    fn field_docs(&self) -> &'static [super::kinddoc::FieldDoc] {
        use super::kinddoc::FieldDoc;
        &[
        FieldDoc { name: CONTEXT_KEYS.steps, ty: "array of object", meaning: "ordered steps, each naming a `tool` and an `args` mapping (`$.input`, `$.prev.result`, `$.steps[i].result` paths or literals)" },
        ]
    }

    fn ceiling_fields(&self) -> &'static [&'static str] {
        &["budget.max_child_calls", "budget.max_wall_ms", "chain_fanout_max"]
    }

    fn contract(&self) -> &'static KindContract {
        &CONTRACT
    }

    fn name(&self) -> &'static str {
        "chain"
    }

    fn validate(&self, spec: &Value) -> Result<(), KindError> {
        parse_steps(spec)?;
        spec_declared_outputs(spec).map(|_| ())
    }

    fn declared_outputs(&self, spec: &Value) -> Vec<OutputDecl> {
        spec_declared_outputs(spec).unwrap_or_default()
    }

    /// PRD-mcphost-spec-unknown-field-rejection requirement 1/4: `steps` is
    /// the only top-level field [`parse_steps`] reads today -- per-step key
    /// checking (`tool`/`args`/`on_error`, requirement 7) is a deferred P1
    /// follow-up, not this field list's concern. Kept a plain literal:
    /// `scripts/spec-fields-doc-check.sh` parses it straight out of this
    /// file (the chain SPEC's field, not a context key).
    fn known_spec_fields(&self) -> &'static [&'static str] {
        &["steps"]
    }

    fn required_spec_field(&self) -> Option<&'static str> {
        Some(CONTEXT_KEYS.steps)
    }

    /// PRD-mcphost-chain-run-lineage requirement 1: `input_schema` is
    /// derived from every step's `$.input.<name>` mapping path
    /// ([`chain_input_schema`]) -- exactly `{"type": "object"}` (unchanged
    /// from before this PRD) for a chain with none. `_spec` failing to
    /// parse here can't happen for an already-published tool (`validate`
    /// already rejected it at publish time), so the fallback is only ever
    /// exercised by a caller that skips validation entirely (this kind's
    /// own unit tests) -- same permissive schema a parse failure would have
    /// produced before this PRD existed.
    fn describe(&self, spec: &Value) -> ToolDescriptor {
        let input_schema = parse_steps(spec)
            .map(|steps| chain_input_schema(&steps))
            .unwrap_or_else(|_| json!({"type": "object"}));
        ToolDescriptor {
            name: "chain".to_string(),
            description: "Runs a fixed sequence of this tenant's tools in order, passing each step's mapped result into the next.".to_string(),
            input_schema,
        }
    }

    /// PRD-mcphost-chain-run-lineage requirement 3: `chain`'s own call-time
    /// pre-check (below) reports every missing `$.input.*` at once
    /// (`compose_input_missing`) -- if the generic dispatch-time validator
    /// also enforced `describe()`'s new `input_schema.required`, a call
    /// missing an input would fail `args_invalid` (naming only the first
    /// missing field) before ever reaching this kind's own `call`, making
    /// `compose_input_missing` unreachable. `host.tool_test`'s dry run
    /// (`ctx.test_mode`) needs the same carve-out to resolve what it can
    /// from an incomplete `call_args` rather than refusing outright.
    fn validates_own_args(&self) -> bool {
        true
    }

    async fn call(&self, spec: &Value, args: Value, ctx: &CallCtx) -> Result<Value, KindError> {
        let steps = parse_steps(spec)?;

        // Requirement 3 / AC8: a dry run resolves what it can and executes
        // no ordinary tenant-tool step (AC5: an allowlisted host step is the
        // one exception -- see `dry_run_report`'s own doc comment).
        if ctx.test_mode {
            return dry_run_report(&steps, &args, ctx).await;
        }

        // PRD-mcphost-chain-run-lineage requirement 3 (AC3): refuse before
        // step 1 ever dispatches if any `$.input.*` path a step references
        // is missing from `args` -- naming every missing field (not just
        // the first) and the first step that actually needs one of them.
        // Zero steps run, so [`compose_call`] never gets a chance to write
        // a child run row for this call at all (requirement 4's "one row
        // per EXECUTED step").
        let input_refs = collect_input_refs(&steps);
        let mut missing: Vec<&str> = Vec::new();
        let mut first_missing: Option<&InputRef> = None;
        for r in &input_refs {
            if args.get(r.name.as_str()).is_none() {
                missing.push(&r.name);
                if first_missing.is_none() {
                    first_missing = Some(r);
                }
            }
        }
        if let Some(first) = first_missing {
            let tool_name = ctx.tool_name.as_deref().unwrap_or("chain");
            return Err(KindError::structured_with(
                codes::COMPOSE_INPUT_MISSING,
                format!(
                    "chain '{tool_name}' needs input(s) [{}] (used by step {} '{}')",
                    missing.join(", "),
                    first.step_no,
                    first.tool,
                ),
                json!({"missing": missing, "step": first.step_no, "tool": first.tool}),
            ));
        }

        let mut steps_trace: Vec<Value> = Vec::with_capacity(steps.len());
        let mut step_results: Vec<Value> = Vec::with_capacity(steps.len());
        let mut prev: Option<Value> = None;
        let mut failed_steps: Vec<usize> = Vec::new();
        let mut last_ok_result: Option<Value> = None;

        for (i, step) in steps.iter().enumerate() {
            let failed_step_no = i + 1;
            let context = step_context(&args, prev.as_ref(), &step_results);

            if let Some(path) = step.for_each.as_deref() {
                match run_fanout(ctx, step, failed_step_no, &context, path).await {
                    Ok(out) => {
                        let join = out.join;
                        let failed_count = join["failed_count"].as_u64().unwrap_or(0);
                        if let Some((index, code, message)) = out.first_failure
                            && !step.continue_on_error
                        {
                            return Err(KindError::structured_with(
                                codes::COMPOSE_FANOUT_ITEM_FAILED,
                                format!(
                                    "step {failed_step_no} ('{}'): item {index} failed with {code}: {message}",
                                    step.tool
                                ),
                                json!({
                                    "step": failed_step_no,
                                    "index": index,
                                    // The wire envelope owns `data.error_code`
                                    // (it is always the call's own code), so
                                    // the item's code travels under its own key.
                                    "item_error_code": code,
                                    "message": message,
                                    "failed_step": failed_step_no,
                                    "step_tool": step.tool,
                                }),
                            ));
                        }
                        let mut entry = json!({
                            "step": failed_step_no,
                            "tool": step.tool,
                            "status": if failed_count == 0 { "done" } else { "error" },
                            (CONTEXT_KEYS.result): join.clone(),
                            "fanout": {"path": path, "concurrency": out.concurrency},
                        });
                        if failed_count > 0 {
                            failed_steps.push(failed_step_no);
                            entry["error_class"] = json!(codes::COMPOSE_FANOUT_ITEM_FAILED);
                        }
                        steps_trace.push(entry);
                        step_results.push(join.clone());
                        prev = Some(join.clone());
                        last_ok_result = Some(join);
                    }
                    Err(e) => {
                        let (code, _, _) = kind_error_parts(&e);
                        // The plan ceiling is a refusal of the whole call,
                        // never something `on_error: continue` swallows.
                        if step.continue_on_error && code != codes::COMPOSE_FANOUT_EXCEEDED {
                            failed_steps.push(failed_step_no);
                            steps_trace.push(json!({
                                "step": failed_step_no,
                                "tool": step.tool,
                                "status": "error",
                                "error_class": code,
                            }));
                            step_results.push(Value::Null);
                            prev = None;
                            continue;
                        }
                        return Err(e);
                    }
                }
                continue;
            }

            let resolved_args = match if step.prev_whole {
                prev.clone().ok_or_else(|| "$.prev".to_string())
            } else {
                resolve_args(&step.args, &context)
            } {
                Ok(v) => v,
                Err(path) => {
                    // PRD-mcphost-chain-prev-contract P0 requirement 5: a
                    // `$.prev`/`$.steps[i]` miss names the fields the
                    // predecessor's result actually carries, and the
                    // corrected path when one matches.
                    let mut data = Map::new();
                    data.insert("path".to_string(), json!(path));
                    data.insert("failed_step".to_string(), json!(failed_step_no));
                    data.insert("step_tool".to_string(), json!(step.tool));
                    let mut hint = String::new();
                    if let Some(r) = StepRef::parse(&path, failed_step_no)
                        && let Some(base) = r.base_result(&context)
                    {
                        let evidence = actual_result_evidence(&r, base);
                        hint = format!("; available: {}", evidence["available"]);
                        if let Some(d) = evidence.get("did_you_mean") {
                            hint.push_str(&format!("; did you mean '{}'", d.as_str().unwrap_or_default()));
                        }
                        data.extend(evidence);
                    }
                    let err = KindError::structured_with(
                        codes::COMPOSE_MAPPING_MISSING,
                        format!(
                            "step {failed_step_no} ('{}'): mapping path '{path}' resolved to nothing{hint}",
                            step.tool
                        ),
                        Value::Object(data),
                    );
                    if step.continue_on_error {
                        failed_steps.push(failed_step_no);
                        super::record_undispatched_step(
                            ctx,
                            ctx.tenant_id,
                            &step.tool,
                            failed_step_no as i64,
                            codes::COMPOSE_MAPPING_MISSING,
                        )
                        .await;
                        steps_trace.push(json!({
                            "step": failed_step_no,
                            "tool": step.tool,
                            "status": "error",
                            "error_class": codes::COMPOSE_MAPPING_MISSING,
                        }));
                        // Requirement 3: a step whose own mapping never
                        // resolved contributes no result downstream --
                        // `$.prev`/`$.steps[i]` referring to it will
                        // likewise fail to resolve on a later step, unless
                        // that later step also tolerates the miss.
                        step_results.push(Value::Null);
                        prev = None;
                        continue;
                    }
                    return Err(err);
                }
            };

            match compose_call_ref(
                ctx,
                ctx.tenant_id,
                &step.tool,
                resolved_args.clone(),
                None,
                // PRD-mcphost-chain-run-lineage requirement 8 (AC11): this
                // step's own 1-based position, so a composed child run row
                // (if lineage is active) carries `step_no`.
                Some(failed_step_no as i64),
                None,
                // PRD-mcphost-jq-kind R3: the child reads this as `$prev`.
                prev.clone(),
            )
            .await
            {
                Ok(result) => {
                    steps_trace.push(json!({
                        "step": failed_step_no,
                        "tool": step.tool,
                        "status": "done",
                        // PRD-mcphost-chain-host-steps AC1: a step's own
                        // result is now in its own trace entry, not only
                        // reachable as the chain's overall `result` (the
                        // last step's own) or via `$.steps[i].result` inside
                        // a LATER step's own mapping -- a caller inspecting
                        // the finished call's trace directly (no further
                        // step to map through) can read any step's result,
                        // not only the last one.
                        (CONTEXT_KEYS.result): result.clone(),
                    }));
                    step_results.push(result.clone());
                    prev = Some(result.clone());
                    last_ok_result = Some(result);
                }
                Err(e) => {
                    let (code, message, mut data) = match &e {
                        KindError::Structured { code, message, data } => {
                            (*code, message.clone(), data.clone())
                        }
                        KindError::InvalidSpec(m) => ("invalid_spec", m.clone(), Value::Null),
                        KindError::InvalidArgs(m) => ("args_invalid", m.clone(), Value::Null),
                        KindError::Exec(m) => ("exec", m.clone(), Value::Null),
                    };
                    if let Some(obj) = data.as_object_mut() {
                        obj.insert("failed_step".to_string(), json!(failed_step_no));
                        obj.insert("step_tool".to_string(), json!(step.tool));
                    } else {
                        data = json!({"failed_step": failed_step_no, "step_tool": step.tool});
                    }
                    let wrapped = KindError::structured_with(code, message, data);

                    if step.continue_on_error {
                        failed_steps.push(failed_step_no);
                        steps_trace.push(json!({
                            "step": failed_step_no,
                            "tool": step.tool,
                            "status": "error",
                            "error_class": code,
                        }));
                        step_results.push(Value::Null);
                        prev = None;
                        continue;
                    }
                    return Err(wrapped);
                }
            }
        }

        // Requirement 3: the chain's result is the last step's result.
        let result = last_ok_result.unwrap_or(Value::Null);
        let declared = spec_declared_outputs(spec)?;
        let payload = project_outputs(&declared, &args, prev.as_ref(), &step_results, &result);
        let mut out = json!({
            (CONTEXT_KEYS.result): result,
            (CONTEXT_KEYS.steps): steps_trace,
            "failed_steps": failed_steps,
        });
        if let (Some(payload), Some(obj)) = (payload, out.as_object_mut()) {
            obj.insert("payload".to_string(), payload);
        }
        Ok(out)
    }

    fn payload_from_call_result<'a>(&self, call_result: &'a Value) -> Option<&'a Value> {
        // A chain that declares `outputs` carries the projected `payload`
        // beside `result`; one that declares none keeps the last step's
        // result as its payload.
        call_result.get("payload").or_else(|| call_result.get(CONTEXT_KEYS.result))
    }

    fn example(&self) -> KindExample {
        // PRD-mcphost-chain-host-steps requirement 5 (AC5): the quickstart's
        // own fixed recommendation -- the python starter
        // (`control::STARTER_TOOL_NAME`, referenced by name only; this
        // module can't depend on that tool actually existing) followed by
        // `host.table.append` -- uses tools that exist, unlike the prior
        // `fetch_rows`/`write_rows` placeholders this PRD's own grounding
        // incident traces to. `host.quickstart kind=chain` resolves on any
        // tenant that already published that starter (AC5); the dynamic
        // "this tenant's own first two tools" override in `handler.rs`
        // takes precedence once a tenant has two non-chain tools of its
        // own, same as before this PRD.
        KindExample {
            spec: json!({
                (CONTEXT_KEYS.steps): [
                    {"tool": crate::control::STARTER_TOOL_NAME, "args": {"text": "$.input.text"}},
                    {"tool": "host.table.append", "args": {"table": "runs", "rows": "$.prev.result.appended"}},
                    // PRD-mcphost-chain-fanout-and-join R3: one step per
                    // element of a list the caller passes, `$.item` being
                    // the element; capped by `limits.plan.chain_fanout_max`.
                    {"tool": crate::control::STARTER_TOOL_NAME, "args": {"note": "$.item"}, "for_each": "$.input.notes"}
                ]
            }),
            call_args: json!({"text": "hello", "notes": ["first", "second"]}),
            // PRD-mcphost-tool-test-truth P1 requirement 2 (AC7): the
            // mapping above is now `$.prev.result.<field>` (not the bare
            // `$.prev.result` every earlier version of this example used),
            // and this blurb names `verdict` -- the field `host.tool_test`'s
            // dry run now reports for every `$.prev`/`$.steps[i]` mapping
            // (`pass`, `will_fail` naming the predecessor's actual declared
            // outputs, or `unverifiable` naming the next call).
            blurb: format!("steps run in order; each step's args may pull from $.input (this call's own args), $.prev (the previous step's result, e.g. $.prev.result.<field>), or $.steps[i] (any earlier step's result by 0-based index). A step may also name an allowlisted host.* verb (host.quickstart's own host_steps_allowed) -- it runs under this chain's own tenant, metered as one step. host.tool_test's dry run reports a verdict (pass, will_fail, or unverifiable) for every $.prev/$.steps mapping. A step with for_each (a mapping path to an array) runs once per element with $.item and $.index in its args, at most limits.plan.chain_fanout_max items, and returns {{items, failed, ok, failed_count}}. {}", grammar_sentence()),
        }
    }
}

crate::err_code! {
    pub FIXTURE_SHAPE_MISMATCH,
    code: "fixture_shape_mismatch", family: Spec, class: AgentFault, retryable: false,
    remedy: "Give the fixture one of the declared outputs named in data.expected.",
}

crate::err_code! {
    pub FIXTURE_STEP_AMBIGUOUS,
    code: "fixture_step_ambiguous", family: Spec, class: AgentFault, retryable: false,
    remedy: "Key the fixture by step number; several steps use that tool.",
}

crate::err_code! {
    pub FIXTURE_STEP_UNKNOWN,
    code: "fixture_step_unknown", family: Spec, class: AgentFault, retryable: false,
    remedy: "Key the fixture by a step that exists; data.steps is the step count.",
}

crate::err_code! {
    pub STEP_TOOL_NOT_ALLOWED,
    code: "step_tool_not_allowed", family: Spec, class: AgentFault, retryable: false,
    remedy: "Use a host step from the allowlist named in the message.",
}

crate::err_code! {
    pub STEP_TOOL_NOT_FOUND,
    code: "step_tool_not_found", family: Spec, class: AgentFault, retryable: false,
    remedy: "Use a tool that exists; data.suggestions lists near matches.",
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::kinds::CallCtx;

    fn spec_two_steps() -> Value {
        json!({
            "steps": [
                {"tool": "a", "args": {"x": "$.input.x"}},
                {"tool": "b", "args": {"y": "$.prev.result.payload.y"}}
            ]
        })
    }

    #[test]
    fn validate_rejects_missing_steps() {
        let kind = ChainKind;
        let err = kind.validate(&json!({})).unwrap_err();
        assert!(matches!(err, KindError::InvalidSpec(_)));
    }

    #[test]
    fn validate_rejects_empty_steps() {
        let kind = ChainKind;
        let err = kind.validate(&json!({"steps": []})).unwrap_err();
        assert!(matches!(err, KindError::InvalidSpec(_)));
    }

    #[test]
    fn validate_rejects_a_step_missing_tool() {
        let kind = ChainKind;
        let err = kind
            .validate(&json!({"steps": [{"args": {}}]}))
            .unwrap_err();
        assert!(matches!(err, KindError::InvalidSpec(_)));
    }

    #[test]
    fn validate_rejects_a_bad_on_error_value() {
        let kind = ChainKind;
        let err = kind
            .validate(&json!({"steps": [{"tool": "a", "on_error": "retry"}]}))
            .unwrap_err();
        assert!(matches!(err, KindError::InvalidSpec(_)));
    }

    #[test]
    fn validate_accepts_a_well_formed_two_step_spec() {
        let kind = ChainKind;
        kind.validate(&spec_two_steps()).expect("valid spec"); // allowlist: test-only expect inside #[cfg(test)]
    }

    #[tokio::test]
    async fn call_without_compose_wiring_reports_unavailable_not_a_panic() {
        // A `chain` call reached through a `CallCtx` with no
        // `compose_db`/`compose_kinds` (e.g. `host.tool_run`, today) must
        // fail cleanly rather than panic. `compose_call`'s own refusal is a
        // plain `KindError::Exec`, but this kind's step-dispatch loop wraps
        // *every* step failure (whatever its original variant) into a
        // `Structured` error carrying `failed_step`/`step_tool` -- AC6/AC7
        // need that shape regardless of which underlying error caused it,
        // so the wrapped shape here (not a bare `Exec`) is the intended
        // behavior, not a bug.
        let kind = ChainKind;
        let spec = json!({"steps": [{"tool": "a", "args": {}}]});
        let ctx = CallCtx::for_test(1, "t_deadbeef");
        let err = kind.call(&spec, json!({}), &ctx).await.unwrap_err();
        match err {
            KindError::Structured { code, data, .. } => {
                assert_eq!(code, "exec");
                assert_eq!(data["failed_step"], json!(1));
                assert_eq!(data["step_tool"], json!("a"));
            }
            other => panic!("expected a wrapped Structured(\"exec\") error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dry_run_resolves_input_paths_and_flags_prev_as_unresolved() {
        let steps = parse_steps(&spec_two_steps()).expect("parse"); // allowlist: test-only expect inside #[cfg(test)]
        let ctx = CallCtx::for_test(1, "t_deadbeef");
        let report = dry_run_report(&steps, &json!({"x": 42}), &ctx).await.expect("report");
        let steps_out = report["steps"].as_array().expect("steps array"); // allowlist: test-only expect inside #[cfg(test)]
        assert_eq!(steps_out[0]["resolved_args"]["x"], json!(42));
        assert!(steps_out[1]["resolved_args"]["y"]["unresolved_path"].is_string());
    }
}
