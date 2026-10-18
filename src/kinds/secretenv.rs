//! Secret and environment plumbing shared by the sandboxed source kinds
//! (`python`, `js`): resolving a spec's `secrets` into `SECRET_<NAME>`
//! environment entries, merging in plain `env`, rendering the environment
//! `host.tool_test` shows, and scrubbing secret values from anything that
//! leaves the host. One copy (PRD-mcphost-js-kind R5).

use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::{KindError, SecretResolver};

/// A spec's declared `secrets`, resolved against the tenant's stored values
/// and named `SECRET_<NAME>` for the sandboxed process.
pub(crate) fn secret_env(secrets: &[String], resolver: &dyn SecretResolver) -> Vec<(String, String)> {
    secrets
        .iter()
        .filter_map(|name| {
            resolver
                .resolve(name)
                .map(|value| (format!("SECRET_{}", name.to_ascii_uppercase()), value))
        })
        .collect()
}

/// PRD-mcphost-python-kind-plain-env requirement 1 (AC1/AC6): `secret_env`
/// (already `SECRET_<NAME>`-prefixed) plus this spec's own plain `env`
/// entries (verbatim names), the exact set of extra environment variables
/// every sandbox spawn site (`call`'s cold path, `maybe_promote_to_warm`,
/// `tool_run`) hands to `sandbox::RunSpec::extra_env`.
pub(crate) fn merge_env(secret_env: &[(String, String)], env: &[(String, String)]) -> Vec<(String, String)> {
    let mut combined = secret_env.to_vec();
    combined.extend(env.iter().cloned());
    // PRD-mcphost-document-store P1 requirement 6: every sandboxed call
    // carries this so a tool can `os.environ["MCPHOST_DOCS_ENDPOINT"]` to
    // detect it's running inside mcphost before calling `mcphost.docs.get`
    // -- the value itself names no reachable network address (the real
    // channel is the same in-process stdin/stdout bridge `mcphost.state`
    // uses, see `DocsSidecarBridge`), same as this crate's other
    // `MCPHOST_*`-reserved, host-injected (never tool-declared -- see
    // `env_name_reason_if_reserved`) environment entries.
    combined.push(("MCPHOST_DOCS_ENDPOINT".to_string(), "stdio://mcphost.docs".to_string()));
    combined
}

/// PRD-mcphost-secret-non-exfiltration-proof R2: a sandboxed tool's result
/// that carries one of its `SECRET_<NAME>` values fails `secret_leak_blocked
/// {where: "result"}` instead of being returned redacted.
pub(crate) fn block_result_leak(value: &Value, secret_env: &[(String, String)]) -> Result<(), KindError> {
    for (env_name, secret) in secret_env {
        if crate::leakguard::value_contains_secret(value, secret) {
            let name = env_name.strip_prefix("SECRET_").unwrap_or(env_name).to_ascii_lowercase();
            return Err(crate::leakguard::blocked("result", &name, json!({})));
        }
    }
    Ok(())
}

pub(crate) fn redact_str(input: &str, secrets: &[String]) -> String {
    let mut ordered: Vec<&String> = secrets.iter().filter(|s| !s.is_empty()).collect();
    ordered.sort_by_key(|s| std::cmp::Reverse(s.len()));
    let mut out = input.to_string();
    for secret in ordered {
        out = crate::leakguard::redact_secret(&out, secret);
    }
    out
}

pub(crate) fn redact_value(value: &Value, secrets: &[String]) -> Value {
    if secrets.is_empty() {
        return value.clone();
    }
    match value {
        Value::String(s) => Value::String(redact_str(s, secrets)),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), redact_value(v, secrets)))
                .collect(),
        ),
        Value::Array(arr) => Value::Array(arr.iter().map(|v| redact_value(v, secrets)).collect()),
        other => other.clone(),
    }
}

pub(crate) fn redact_kind_error(err: KindError, secrets: &[String]) -> KindError {
    if secrets.is_empty() {
        return err;
    }
    match err {
        KindError::Structured {
            code,
            message,
            data,
        } => KindError::Structured {
            code,
            message: redact_str(&message, secrets),
            data: redact_value(&data, secrets),
        },
        other => other,
    }
}

/// PRD-mcphost-python-kind-plain-env requirement 5 (AC5): `host.tool_test`'s
/// one rendered-environment listing -- every plain `env` entry shown
/// verbatim (`kind: "env"`), every declared secret name shown redacted
/// (`kind: "secret"`, value always `"***"`, regardless of whether this
/// tenant has actually set it -- a missing secret is still a secret, not
/// something to reveal the absence of by omission). Sorted by name
/// (`env` is a `BTreeMap`; `secrets` is sorted here) so the listing's order
/// is deterministic and independent of publish-time field order.
pub(crate) fn environment_report(env: &BTreeMap<String, String>, secrets: &[String]) -> Value {
    let mut entries: Vec<Value> = env
        .iter()
        .map(|(name, value)| json!({"name": name, "kind": "env", "value": value}))
        .collect();
    let mut sorted_secrets = secrets.to_vec();
    sorted_secrets.sort();
    entries.extend(
        sorted_secrets
            .iter()
            .map(|name| json!({"name": name, "kind": "secret", "value": "***"})),
    );
    json!(entries)
}

