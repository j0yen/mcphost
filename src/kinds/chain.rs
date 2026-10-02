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
//! - Real child-run rows (`host.runs.get` inlining children, `trigger`
//!   values) wait on PRD-mcphost-runs-and-jobs, not built yet -- this
//!   kind's own result carries a `steps` trace (`tool`, `status`,
//!   `error_class`) as the best available stand-in.
//! - `on_error: "continue"` (requirement 3) is implemented (requirement 3 /
//!   AC7); `map` steps (P2) are not.

use serde_json::{Map, Value, json};

use super::{CallCtx, HOST_STEPS_ALLOWED, Kind, KindError, KindExample, Path, ToolDescriptor, compose_call};
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
/// each step's resolved arguments, without ever dispatching a step. A
/// mapping that reaches into `$.prev`/`$.steps[i]` can't be resolved for
/// real (no step has run), so it's reported as `{"unresolved_path": "$..."}`
/// instead of a value; `$.input.*` and literals resolve exactly as a real
/// call would.
///
/// PRD-mcphost-chain-host-steps requirement 4/AC4: each step also carries
/// `resolved` (`"host"` for an allowlisted `host.*` verb, `"tenant"` for a
/// sibling tool -- a step here was already resolved once, at publish time
/// (`resolve_steps`), so this is a pure syntactic re-derivation, no DB
/// lookup needed) and, for a `"host"` step, `side_effects: true` -- until
/// PRD-mcphost-dry-run-side-effects lands, a dry run never dispatches ANY
/// step (host or tenant), so a host step's own real-world write (e.g.
/// `host.table.append`'s row) would NOT happen during this report; the flag
/// says so explicitly rather than leaving an agent to assume dry-run safety
/// it doesn't have yet for that step.
fn dry_run_report(steps: &[ParsedStep], call_args: &Value) -> Value {
    // Only `input` is available before anything has run.
    let context = json!({"input": call_args, "prev": Value::Null, "steps": []});
    let report: Vec<Value> = steps
        .iter()
        .enumerate()
        .map(|(i, step)| {
            let mut resolved = Map::with_capacity(step.args.len());
            for (k, v) in &step.args {
                let value = match v {
                    Value::String(s) if s.starts_with("$.") => {
                        match Path::parse(s).ok().and_then(|p| p.resolve(&context).cloned()) {
                            Some(v) => v,
                            None => json!({"unresolved_path": s}),
                        }
                    }
                    other => other.clone(),
                };
                resolved.insert(k.clone(), value);
            }
            let is_host = HOST_STEPS_ALLOWED.contains(&step.tool.as_str());
            let mut entry = json!({
                "step": i + 1,
                "tool": step.tool,
                "resolved_args": resolved,
                "resolved": if is_host { "host" } else { "tenant" },
            });
            if is_host
                && let Value::Object(map) = &mut entry
            {
                map.insert("side_effects".to_string(), json!(true));
            }
            entry
        })
        .collect();
    json!({"dry_run": true, "steps": report})
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

    fn describe(&self, _spec: &Value) -> ToolDescriptor {
        ToolDescriptor {
            name: "chain".to_string(),
            description: "Runs a fixed sequence of this tenant's tools in order, passing each step's mapped result into the next.".to_string(),
            // Requirement 4: a step's mapping may reach `$.input.<anything>`
            // -- the chain's own call args have no fixed shape the chain
            // itself can declare, so this stays maximally permissive.
            input_schema: json!({"type": "object"}),
        }
    }

    async fn call(&self, spec: &Value, args: Value, ctx: &CallCtx) -> Result<Value, KindError> {
        let steps = parse_steps(spec)?;

        // Requirement 3 / AC8: a dry run resolves what it can and executes
        // nothing.
        if ctx.test_mode {
            return Ok(dry_run_report(&steps, &args));
        }

        let mut steps_trace: Vec<Value> = Vec::with_capacity(steps.len());
        let mut step_results: Vec<Value> = Vec::with_capacity(steps.len());
        let mut prev: Option<Value> = None;
        let mut failed_steps: Vec<usize> = Vec::new();
        let mut last_ok_result: Option<Value> = None;

        for (i, step) in steps.iter().enumerate() {
            let failed_step_no = i + 1;
            let steps_context: Vec<Value> = step_results
                .iter()
                .map(|r| json!({"result": r}))
                .collect();
            let context = json!({
                "input": &args,
                "prev": prev.as_ref().map(|r| json!({"result": r})),
                "steps": steps_context,
            });

            let resolved_args = match resolve_args(&step.args, &context) {
                Ok(v) => v,
                Err(path) => {
                    let err = KindError::structured_with(
                        "compose_mapping_missing",
                        format!(
                            "step {failed_step_no} ('{}'): mapping path '{path}' resolved to nothing",
                            step.tool
                        ),
                        json!({
                            "path": path,
                            "failed_step": failed_step_no,
                            "step_tool": step.tool,
                        }),
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

            match compose_call(ctx, ctx.tenant_id, &step.tool, resolved_args.clone(), None).await {
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
                    {"tool": "host.table.append", "args": {"table": "runs", "rows": "$.prev.result"}}
                ]
            }),
            call_args: json!({"text": "hello"}),
            blurb: "steps run in order; each step's args may pull from $.input (this call's own args), $.prev (the previous step's result), or $.steps[i] (any earlier step's result by 0-based index). A step may also name an allowlisted host.* verb (host.quickstart's own host_steps_allowed) -- it runs under this chain's own tenant, metered as one step.".to_string(),
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

    #[test]
    fn dry_run_resolves_input_paths_and_flags_prev_as_unresolved() {
        let steps = parse_steps(&spec_two_steps()).expect("parse"); // allowlist: test-only expect inside #[cfg(test)]
        let report = dry_run_report(&steps, &json!({"x": 42}));
        let steps_out = report["steps"].as_array().expect("steps array"); // allowlist: test-only expect inside #[cfg(test)]
        assert_eq!(steps_out[0]["resolved_args"]["x"], json!(42));
        assert!(steps_out[1]["resolved_args"]["y"]["unresolved_path"].is_string());
    }
}
