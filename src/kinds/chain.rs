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
//! - `outputs` (promoting named steps' fields into the chain's own result)
//!   is accepted at publish time but not yet applied -- the chain's result
//!   is always its last step's result, matching the *default* behavior
//!   `outputs` would only override.
//! - `on_error: "continue"` (requirement 3) is implemented (requirement 3 /
//!   AC7); `map` steps (P2) are not.
//!
//! PRD-mcphost-chain-run-lineage (requirements 1-3): real child-run rows
//! (`host.runs.get` inlining children, `trigger: "composition"`) are now
//! written by [`compose_call`] itself for every step dispatch below -- this
//! kind's own `steps` trace stays (existing callers of the chain's result
//! shape are unaffected), it's no longer the only record of what ran.
//! `describe()` now derives `input_schema.required` from every step's
//! `$.input.<name>` mapping paths, and [`ChainKind::call`] refuses a call
//! missing one of them (`compose_input_missing`) before step 1 ever
//! dispatches -- see [`super::Kind::validates_own_args`]'s own doc comment
//! for why the generic dispatch-time schema check has to step aside for
//! this kind specifically.

use serde_json::{Map, Value, json};

use super::{
    CallCtx, HOST_STEPS_ALLOWED, Kind, KindError, KindExample, KindRegistry, Path, ToolDescriptor, compose_call,
};
use crate::errors::AppError;

pub struct ChainKind;

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
/// and `host.spec_test` time (never from [`compose_call`]'s own, narrower,
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
                code: "step_tool_not_allowed",
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
            code: "step_tool_not_found",
            message: format!("step {step_no} ('{}'): no such tool", step.tool),
            data: json!({"step": step_no, "name": step.tool, "suggestions": suggestions}),
        });
    }
    Ok(())
}

/// PRD-mcphost-chain-prev-contract P0 requirement 1: the ONE constructor of
/// the context a step's mappings resolve against -- `{"input": <chain args>,
/// "prev": {"result": <predecessor result>} | null, "steps": [{"result":
/// ...}, ...]}`. [`ChainKind::call`] and [`dry_run_report`] both build their
/// context here, and [`accepted_prev_keys`]/[`accepted_step_keys`] read the
/// checker's accepted first-level keys back out of a context this function
/// built, so the two cannot drift apart.
pub fn step_context(input: &Value, prev: Option<&Value>, steps: &[Value]) -> Value {
    json!({
        "input": input,
        "prev": prev.map(|r| json!({"result": r})),
        "steps": steps.iter().map(|r| json!({"result": r})).collect::<Vec<_>>(),
    })
}

fn object_keys(v: &Value) -> std::collections::BTreeSet<String> {
    v.as_object().map(|m| m.keys().cloned().collect()).unwrap_or_default()
}

/// The first-level keys a mapping may use under `$.prev` (today `{result}`),
/// read from a sample context [`step_context`] built.
pub fn accepted_prev_keys() -> std::collections::BTreeSet<String> {
    object_keys(&step_context(&Value::Null, Some(&Value::Null), &[])["prev"])
}

/// The first-level keys a mapping may use under `$.steps[i]` (today
/// `{result}`), read from a sample context [`step_context`] built.
pub fn accepted_step_keys() -> std::collections::BTreeSet<String> {
    object_keys(&step_context(&Value::Null, None, &[Value::Null])["steps"][0])
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
            &context["prev"]
        } else {
            &context["steps"][self.predecessor_no - 1]
        };
        holder.get("result").filter(|v| !v.is_null())
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

async fn check_step_ref(
    db: Option<&crate::db::Db>,
    kinds: Option<&KindRegistry>,
    tenant_id: i64,
    predecessor_tool: &str,
    r: &StepRef,
) -> RefCheck {
    let declared = match (db, kinds) {
        (Some(db), Some(kinds)) => declared_output_names(db, kinds, tenant_id, predecessor_tool)
            .await
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let mut evidence = Map::new();
    if !declared.is_empty() {
        evidence.insert("available".to_string(), json!(declared));
        evidence.insert("predecessor_keys".to_string(), json!(declared));
    }
    let Some(first) = r.name(0) else {
        return RefCheck::Unverifiable;
    };
    if !r.accepted_keys().contains(first) {
        // Missing hop: `$.prev.<seg>` should be `$.prev.result.<seg>`; when
        // the predecessor's declared fields name a close match for `<seg>`,
        // point at that field instead.
        let suggestion = match closest_name(first, &declared) {
            Some(near) if !declared.iter().any(|d| d == first) => r.rebuilt(&["result"], Some((0, near))),
            _ => r.rebuilt(&["result"], None),
        };
        evidence.insert("did_you_mean".to_string(), json!(suggestion));
        return RefCheck::WillFail(evidence);
    }
    let Some(field) = r.name(1) else {
        // Whole predecessor value (`$.prev.result`): nothing to check
        // against a schema; Pass only if the predecessor declares one.
        return if declared.is_empty() { RefCheck::Unverifiable } else { RefCheck::Pass };
    };
    if declared.is_empty() {
        return RefCheck::Unverifiable;
    }
    if declared.iter().any(|d| d == field) || super::ENVELOPE_WRAPPER_KEYS.contains(&field) {
        RefCheck::Pass
    } else {
        if let Some(near) = closest_name(field, &declared) {
            evidence.insert("did_you_mean".to_string(), json!(r.rebuilt(&[], Some((1, near)))));
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

async fn steps_verdict(
    db: Option<&crate::db::Db>,
    kinds: Option<&KindRegistry>,
    tenant_id: i64,
    steps: &[ParsedStep],
) -> StepsVerdict {
    let mut failures: Vec<Value> = Vec::new();
    let mut unverifiable_step: Option<usize> = None;
    let mut passes = std::collections::HashSet::new();
    for (i, step) in steps.iter().enumerate() {
        let step_no = i + 1;
        for v in step.args.values() {
            let Value::String(raw) = v else { continue };
            let Some(r) = StepRef::parse(raw, step_no) else {
                continue;
            };
            if r.tokens.is_empty() {
                continue; // whole-value mapping -- nothing to check.
            }
            let predecessor_no = r.predecessor_no;
            let Some(predecessor) = steps.get(predecessor_no - 1) else {
                continue; // out of range -- not this check's concern.
            };
            match check_step_ref(db, kinds, tenant_id, &predecessor.tool, &r).await {
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
    steps_verdict(Some(db), Some(kinds), tenant_id, &steps).await.overall
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
        let declared = declared_output_names(db, kinds, tenant_id, &steps[i - 1].tool)
            .await
            .unwrap_or_default();
        let entry = if declared.is_empty() {
            json!({"step": i + 1, "tool": step.tool, "prev_fields": null,
                   "hint": "predecessor declares no outputs; host.tool_test reports prev_fields from its dry-run result"})
        } else {
            json!({"step": i + 1, "tool": step.tool, "prev_fields": declared, "prev_fields_source": "declared"})
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
        for v in step.args.values() {
            if let Value::String(s) = v
                && let Some(name) = input_arg_name(s)
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
    continue_on_error: bool,
}

fn invalid_spec(message: impl Into<String>) -> KindError {
    KindError::InvalidSpec(message.into())
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
        .get("steps")
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
        let args = match step_obj.get("args") {
            None => Map::new(),
            Some(Value::Object(m)) => m.clone(),
            Some(_) => {
                return Err(invalid_spec(format!(
                    "spec.steps[{i}].args: must be a JSON object"
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
        steps.push(ParsedStep {
            tool,
            args,
            continue_on_error,
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
            closest_name(seg, &available).map(|near| r.rebuilt(&["result"], Some((0, near))))
        }
        (Some("result"), Some(field)) if !available.iter().any(|k| k == field) => {
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

async fn dry_run_report(steps: &[ParsedStep], call_args: &Value, ctx: &CallCtx) -> Value {
    let inputs_required: Vec<String> = collect_input_refs(steps).into_iter().map(|r| r.name).collect();
    let sv = steps_verdict(
        ctx.compose_db.as_ref(),
        ctx.compose_kinds.as_ref(),
        ctx.tenant_id,
        steps,
    )
    .await;
    if sv.overall["verdict"] == json!("will_fail") {
        return json!({
            "chain_dry_run": true,
            "steps": [],
            "inputs_required": inputs_required,
            "verdict": "will_fail",
            "failures": sv.overall["failures"].clone(),
            "evidence": sv.overall["failures"].clone(),
        });
    }

    // Steps whose actual dry-run result some later mapping reads and the
    // static check could not already settle from declared `outputs`.
    let mut needed: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for (i, step) in steps.iter().enumerate() {
        for v in step.args.values() {
            if let Value::String(raw) = v
                && let Some(r) = StepRef::parse(raw, i + 1)
                && !r.tokens.is_empty()
                && !sv.passes.contains(&(i + 1, raw.clone()))
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
    for (i, step) in steps.iter().enumerate() {
        let step_no = i + 1;
        let context = step_context(call_args, prev.as_ref(), &step_results);
        let mut resolved = Map::with_capacity(step.args.len());
        let mut unresolved = false;
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
            resolved.insert(k.clone(), value);
        }
        let mut prev_fields = None;
        if i > 0
            && let Some(Value::Object(m)) = prev.as_ref()
        {
            prev_fields = Some(m.keys().cloned().collect::<Vec<_>>());
        }
        let is_host = HOST_STEPS_ALLOWED.contains(&step.tool.as_str());
        if is_host
            && !unresolved
            && let Some(host) = ctx.host_dispatch.as_ref()
        {
            match host.call(&step.tool, Value::Object(resolved.clone())).await {
                Ok(result) => {
                    let mut entry = json!({
                        "step": step_no,
                        "tool": step.tool,
                        "resolved_args": resolved,
                        "resolved": "host",
                        "side_effects": true,
                        "dispatched": true,
                        "result": result,
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
        if !is_host && needed.contains(&step_no) {
            let outcome = if unresolved {
                Err("its own arguments did not resolve".to_string())
            } else {
                dry_run_predecessor(ctx, &step.tool, Value::Object(resolved.clone())).await
            };
            match outcome {
                Ok(result) => {
                    if let Value::Object(map) = &mut entry {
                        map.insert("dispatched".to_string(), json!(true));
                        map.insert("result".to_string(), result.clone());
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
                    not_runnable.insert(step_no, (step.tool.clone(), why));
                }
            }
        }
        report.push(entry);
        step_results.push(Value::Null);
        prev = None;
    }
    let mut out = json!({"chain_dry_run": true, "steps": report, "inputs_required": inputs_required});
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
    out
}

fn insert_prev_fields(entry: &mut Value, prev_fields: &Option<Vec<String>>) {
    if let (Value::Object(map), Some(fields)) = (entry, prev_fields) {
        map.insert("prev_fields".to_string(), json!(fields));
    }
}

#[async_trait::async_trait]
impl Kind for ChainKind {
    fn name(&self) -> &'static str {
        "chain"
    }

    fn validate(&self, spec: &Value) -> Result<(), KindError> {
        parse_steps(spec).map(|_| ())
    }

    /// PRD-mcphost-spec-unknown-field-rejection requirement 1/4: `steps` is
    /// the only top-level field [`parse_steps`] reads today -- per-step key
    /// checking (`tool`/`args`/`on_error`, requirement 7) is a deferred P1
    /// follow-up, not this field list's concern.
    fn known_spec_fields(&self) -> &'static [&'static str] {
        &["steps"]
    }

    fn required_spec_field(&self) -> Option<&'static str> {
        Some("steps")
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
            return Ok(dry_run_report(&steps, &args, ctx).await);
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
                "compose_input_missing",
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

            let resolved_args = match resolve_args(&step.args, &context) {
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
                        "compose_mapping_missing",
                        format!(
                            "step {failed_step_no} ('{}'): mapping path '{path}' resolved to nothing{hint}",
                            step.tool
                        ),
                        Value::Object(data),
                    );
                    if step.continue_on_error {
                        failed_steps.push(failed_step_no);
                        steps_trace.push(json!({
                            "step": failed_step_no,
                            "tool": step.tool,
                            "status": "error",
                            "error_class": "compose_mapping_missing",
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

            match compose_call(
                ctx,
                ctx.tenant_id,
                &step.tool,
                resolved_args.clone(),
                None,
                // PRD-mcphost-chain-run-lineage requirement 8 (AC11): this
                // step's own 1-based position, so a composed child run row
                // (if lineage is active) carries `step_no`.
                Some(failed_step_no as i64),
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
                        "result": result.clone(),
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

        // Requirement 3: the chain's result is the last step's result
        // (`outputs`-based promotion is a deferred follow-up -- see the
        // module doc).
        let result = last_ok_result.unwrap_or(Value::Null);
        Ok(json!({
            "result": result,
            "steps": steps_trace,
            "failed_steps": failed_steps,
        }))
    }

    fn payload_from_call_result<'a>(&self, call_result: &'a Value) -> Option<&'a Value> {
        call_result.get("result")
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
                "steps": [
                    {"tool": crate::control::STARTER_TOOL_NAME, "args": {"text": "$.input.text"}},
                    {"tool": "host.table.append", "args": {"table": "runs", "rows": "$.prev.result.appended"}}
                ]
            }),
            call_args: json!({"text": "hello"}),
            // PRD-mcphost-tool-test-truth P1 requirement 2 (AC7): the
            // mapping above is now `$.prev.result.<field>` (not the bare
            // `$.prev.result` every earlier version of this example used),
            // and this blurb names `verdict` -- the field `host.tool_test`'s
            // dry run now reports for every `$.prev`/`$.steps[i]` mapping
            // (`pass`, `will_fail` naming the predecessor's actual declared
            // outputs, or `unverifiable` naming the next call).
            blurb: format!("steps run in order; each step's args may pull from $.input (this call's own args), $.prev (the previous step's result, e.g. $.prev.result.<field>), or $.steps[i] (any earlier step's result by 0-based index). A step may also name an allowlisted host.* verb (host.quickstart's own host_steps_allowed) -- it runs under this chain's own tenant, metered as one step. host.tool_test's dry run reports a verdict (pass, will_fail, or unverifiable) for every $.prev/$.steps mapping. {}", grammar_sentence()),
        }
    }
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
        let report = dry_run_report(&steps, &json!({"x": 42}), &ctx).await;
        let steps_out = report["steps"].as_array().expect("steps array"); // allowlist: test-only expect inside #[cfg(test)]
        assert_eq!(steps_out[0]["resolved_args"]["x"], json!(42));
        assert!(steps_out[1]["resolved_args"]["y"]["unresolved_path"].is_string());
    }
}
