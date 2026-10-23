//! The host's result envelope: every top-level key a cross-tool wrapper adds
//! to a tool's result, as consts, plus the stamping helpers the wrapper paths
//! insert through and the generated `OUTPUT_ENVELOPE_FIELDS` set the contract
//! publishes (PRD-mcphost-output-envelope-fields-generated).
//!
//! The set is "may carry", not "always carries": `dry_run` and
//! `deprecations` ride only some calls and `__resources_used` is taken off the
//! result at the wire (`_meta.resources_used`) or by the bridge strip.

use serde_json::{Map, Value, json};

/// In-flight marker key a python call's usage rides under until the wire
/// (`call_tool_inner`) moves it to `_meta.resources_used`.
pub const RESOURCES_USED_KEY: &str = "__resources_used";
/// The `calls.id` of the call, a string (`calltrace::attach_call_id`).
pub const CALL_ID_KEY: &str = "call_id";
/// Deprecation notices for a deprecated field/tool the call used.
pub const DEPRECATIONS_KEY: &str = "deprecations";
/// A dry run's report (`host.tool_run`/`host.tool_test`/`host.estimate`).
pub const DRY_RUN_KEY: &str = "dry_run";
/// Wall time of the call in milliseconds (`host.tool_run`).
pub const DURATION_MS_KEY: &str = "duration_ms";
/// Snapshot of the call's `mcphost.state` ops (`host.tool_run`/`tool_test`).
pub const STATE_KEY: &str = "state";

/// Every key a wrapper path may add, sorted. Assembled from the consts above;
/// `contract dump` emits it as `output_envelope_fields`.
pub const OUTPUT_ENVELOPE_FIELDS: &[&str] =
    &[RESOURCES_USED_KEY, CALL_ID_KEY, DEPRECATIONS_KEY, DRY_RUN_KEY, DURATION_MS_KEY, STATE_KEY];

/// `host.tool_run`'s success stamping.
pub fn stamp_tool_run(
    obj: &mut Map<String, Value>,
    duration_ms: i64,
    resources_used: Option<Value>,
    state: Value,
    dry_run: Option<Value>,
) {
    obj.insert(DURATION_MS_KEY.to_string(), json!(duration_ms));
    if let Some(used) = resources_used {
        obj.insert(RESOURCES_USED_KEY.to_string(), used);
    }
    obj.insert(STATE_KEY.to_string(), state);
    if let Some(report) = dry_run {
        obj.insert(DRY_RUN_KEY.to_string(), report);
    }
}

/// `host.tool_test`'s published-tool dry-run stamping.
pub fn stamp_tool_test(
    map: &mut Map<String, Value>,
    state: Value,
    resources_used: Option<Value>,
    dry_run_report: impl FnOnce() -> Value,
) {
    map.insert(STATE_KEY.to_string(), state);
    if let Some(used) = resources_used {
        map.insert(RESOURCES_USED_KEY.to_string(), used);
    }
    default_dry_run(map, dry_run_report);
}

/// A published tool call's usage marker.
pub fn stamp_resources_used(obj: &mut Map<String, Value>, used: Value) {
    obj.insert(RESOURCES_USED_KEY.to_string(), used);
}

/// `call_tool_inner`'s deprecation notices.
pub fn stamp_deprecations(obj: &mut Map<String, Value>, notices: Value) {
    obj.insert(DEPRECATIONS_KEY.to_string(), notices);
}

/// Adds a `dry_run` report unless the callee already set its own (`chain`'s
/// boolean `dry_run: true`).
pub fn default_dry_run(map: &mut Map<String, Value>, report: impl FnOnce() -> Value) {
    map.entry(DRY_RUN_KEY.to_string()).or_insert_with(report);
}

/// `host.estimate` forces `dry_run: true` into the forwarded call's args.
pub fn force_dry_run_arg(args: &mut Map<String, Value>) {
    args.insert(DRY_RUN_KEY.to_string(), json!(true));
}

/// A bridge call's returned JSON stays exactly what the callee produced: the
/// in-flight usage marker is wire-only.
pub fn strip_bridge_resources_used(mut value: Value) -> Value {
    if let Some(obj) = value.as_object_mut() {
        obj.remove(RESOURCES_USED_KEY);
    }
    value
}

/// One cross-tool wrapper path: `apply` runs the same stamping helper the
/// handler calls over a stub tool's output, with fixed inputs.
pub struct WrapperPath {
    pub name: &'static str,
    /// Canonical name of the host tool whose result the path stamps.
    pub tool: &'static str,
    pub apply: fn(Value) -> Value,
}

fn with_obj(mut v: Value, f: impl FnOnce(&mut Map<String, Value>)) -> Value {
    if let Some(obj) = v.as_object_mut() {
        f(obj);
    }
    v
}

fn sample_usage() -> Value {
    json!({"cpu_ms": 1, "peak_rss_mb": 1, "wall_ms": 1, "pids_max_seen": null})
}

/// The six wrapper paths of the PRD's AC 1 universe.
pub fn wrapper_paths() -> Vec<WrapperPath> {
    vec![
        WrapperPath {
            name: "tool_run",
            tool: "host.tool.run",
            apply: |v| {
                with_obj(v, |o| {
                    stamp_tool_run(o, 1, Some(sample_usage()), json!({}), Some(json!({"writes": []})))
                })
            },
        },
        WrapperPath {
            name: "tool_test",
            tool: "host.tool.test",
            apply: |v| with_obj(v, |o| stamp_tool_test(o, json!({}), Some(sample_usage()), || json!({"writes": []}))),
        },
        WrapperPath {
            name: "call_published_tool",
            tool: "host.tool.call",
            apply: |v| with_obj(v, |o| stamp_resources_used(o, sample_usage())),
        },
        WrapperPath {
            name: "attach_call_id",
            tool: "host.tool.call",
            apply: |v| crate::calltrace::attach_call_id(Ok(v), 1).unwrap_or(Value::Null),
        },
        WrapperPath {
            name: "call_tool_inner_deprecations",
            tool: "host.tool.call",
            apply: |v| with_obj(v, |o| stamp_deprecations(o, json!([{"path": "p"}]))),
        },
        WrapperPath {
            name: "forwarding_wrapper",
            tool: "host.estimate",
            apply: |v| with_obj(v, |o| default_dry_run(o, || json!({"writes": []}))),
        },
    ]
}

/// The AC 1 rule for one wrapper path: every key `out` gained beyond the
/// stub's own `x` is either declared by the enclosing tool's output schema or
/// a member of [`OUTPUT_ENVELOPE_FIELDS`]. Returns the keys the path added;
/// `Err` names the offending key and the wrapper path.
pub fn check_added_keys(path: &str, declared: &[String], out: &Value) -> Result<Vec<String>, String> {
    let added: Vec<String> =
        out.as_object().map(|o| o.keys().filter(|k| k.as_str() != "x").cloned().collect()).unwrap_or_default();
    for key in &added {
        if !declared.contains(key) && !OUTPUT_ENVELOPE_FIELDS.contains(&key.as_str()) {
            return Err(format!(
                "wrapper path `{path}` added key `{key}`, which is not in OUTPUT_ENVELOPE_FIELDS (src/envelope.rs): add a const and list it there"
            ));
        }
    }
    Ok(added)
}
