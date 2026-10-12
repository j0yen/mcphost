//! PRD-mcphost-destructive-verb-confirmation-token: the `confirm` token a
//! destructive write verb needs once its dry-run diff reaches the plan's
//! `confirm_rows_min`.
//!
//! The gated set is *derived* from [`crate::write_verbs::WRITE_VERBS`]
//! ([`gated`]): a `Supported` verb of kind delete/drop/revoke/remove. The
//! descriptor annotation, the description suffix ([`CONFIRM_SENTENCE`]),
//! `docs/host-tools.md` (rendered from this file's source text by
//! `scripts/host-tools-doc-check.sh`) and the dispatch gate in
//! `handler::dispatch_tenant_tool_dry_run` all read that one predicate, so
//! a verb added to the registry is gated, annotated and documented with no
//! further code.

use rand::RngCore;
use serde_json::{Map, Value, json};

use crate::errors::AppError;
use crate::write_verbs::{DryRun, Kind, WriteVerb};

/// R2: seconds a minted token stays valid.
pub const TOKEN_TTL_S: i64 = 600;

/// R4: the sentence every gated verb's description ends with and
/// `docs/host-tools.md` repeats once per gated verb. It lives beside the
/// registry in `write_verbs.rs` (the doc check parses it from there).
pub use crate::write_verbs::CONFIRM_SENTENCE;

/// Whether `verb` is confirm-gated: the one predicate behind the dispatch
/// gate, `annotations.destructiveHint` and the description suffix.
pub fn gated(verb: &WriteVerb) -> bool {
    matches!(verb.dry_run, DryRun::Supported)
        && matches!(verb.kind, Kind::Delete | Kind::Drop | Kind::Revoke | Kind::Remove)
}

/// [`gated`] by (possibly aliased) verb name.
pub fn gated_name(name: &str) -> bool {
    crate::write_verbs::lookup(name).is_some_and(gated)
}

/// The dry-run notes' measurable blast radius: rows a delete would remove
/// plus rows inside a table a drop would remove. `None` when the verb
/// noted nothing countable.
pub fn rows_affected(notes: &[(&'static str, Value)]) -> Option<i64> {
    let mut total = None;
    for (class, item) in notes {
        let n = match *class {
            "rows" => item.get("delete").and_then(Value::as_i64),
            "tables" => item.get("rows").and_then(Value::as_i64),
            _ => None,
        };
        if let Some(n) = n {
            *total.get_or_insert(0) += n;
        }
    }
    total
}

/// R1: whether a diff of `notes` needs a token under `confirm_rows_min`
/// (`0` turns the gate off).
pub fn needs_confirm(notes: &[(&'static str, Value)], confirm_rows_min: i64) -> bool {
    confirm_rows_min > 0 && rows_affected(notes).is_some_and(|n| n >= confirm_rows_min)
}

fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            Value::Object(keys.into_iter().map(|k| (k.clone(), canonical(&map[k]))).collect::<Map<_, _>>())
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

fn hash_value(value: &Value) -> String {
    crate::billing::sha256_hex(canonical(value).to_string().as_bytes())
}

/// Canonical-JSON hash of `args` with `confirm` and `dry_run` removed.
pub fn args_hash(args: &Value) -> String {
    let mut args = args.clone();
    if let Some(obj) = args.as_object_mut() {
        obj.remove("confirm");
        obj.remove("dry_run");
    }
    hash_value(&args)
}

/// Hash of the diff a dry run observed (the `would` notes, in call order).
pub fn diff_hash(notes: &[(&'static str, Value)]) -> String {
    hash_value(&Value::Array(notes.iter().map(|(c, v)| json!([c, v])).collect()))
}

/// The `confirm` argument of `args`, if a string.
pub fn confirm_arg(args: &Value) -> Option<&str> {
    args.get("confirm").and_then(Value::as_str)
}

/// A fresh `c_` + 32 random bytes (base64url) token.
pub fn new_token() -> String {
    use base64::Engine;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("c_{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

/// How a token is stored: only its hash.
pub fn token_hash(token: &str) -> String {
    crate::billing::sha256_hex(token.as_bytes())
}

/// The exact dry-run call that mints a token for `args`.
pub fn dry_run_first(tool: &str, args: &Value) -> Value {
    let mut dry = args.clone();
    if let Some(obj) = dry.as_object_mut() {
        obj.remove("confirm");
        obj.insert("dry_run".to_string(), json!(true));
    }
    json!({"tool": tool, "args": dry})
}

/// R1: the refusal a large destructive call without a token gets.
pub fn required_error(tool: &str, args: &Value, would: Value, confirm_rows_min: i64) -> AppError {
    AppError::Structured {
        code: "confirm_required",
        message: format!(
            "{tool} would affect at least {confirm_rows_min} rows or a whole object; call dry_run_first.tool with \
             dry_run_first.args, read `would`, then repeat this call with the returned `confirm`"
        ),
        data: json!({
            "dry_run_first": dry_run_first(tool, args),
            "would": would,
            "confirm_rows_min": confirm_rows_min,
        }),
    }
}

/// R3: the distinct refusal for a token that does not fit; `would` is
/// attached for `state_changed`.
pub fn invalid_error(reason: &'static str, would: Option<Value>) -> AppError {
    let mut data = json!({"reason": reason});
    if let Some(would) = would {
        data["would"] = would;
    }
    AppError::Structured {
        code: "confirm_invalid",
        message: format!("confirm token refused: {reason}"),
        data,
    }
}
