//! PRD-mcphost-dry-run-on-write-verbs: the one registry of host verbs that
//! mutate tenant data, and what each does when called with `dry_run: true`.
//!
//! [`WRITE_VERBS`] is the only hand-written list. [`DRY_RUN_VERBS`] and
//! [`NO_DRY_RUN_VERBS`] are derived from it by `const fn`s, `tools/list`
//! gets its `dry_run` property from it (`handler::host_tools`), the
//! `dry_run` dispatch wrapper (`handler::dispatch_tenant_tool`) reads it, and
//! `docs/host-tools.md` is rendered from the source text of this very table
//! by `scripts/sandbox-api-doc-check.sh`. A write verb that is in none of
//! those fails `tests/dryv_ac02_*` with its name -- prose at a boundary
//! drifts, a derived list cannot.

use serde_json::{Map, Value, json};

use crate::errors::AppError;

/// What a verb does to tenant data -- the `kind` column of the registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Delete,
    Drop,
    Revoke,
    Remove,
    Write,
    Publish,
    Rotate,
}

/// Whether a verb honours `dry_run: true`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DryRun {
    /// Runs inside `dryrun::with_dry_run`, rolled back, answers `would`.
    Supported,
    /// Refuses `dry_run: true` with `args_invalid` + `data.no_dry_run_reason`.
    NoDryRun(&'static str),
}

#[derive(Clone, Copy, Debug)]
pub struct WriteVerb {
    pub name: &'static str,
    pub kind: Kind,
    pub dry_run: DryRun,
}

/// PRD-mcphost-destructive-verb-confirmation-token R4: the sentence every
/// confirm-gated verb's description ends with (`confirm::gated` decides which)
/// and `docs/host-tools.md` repeats once per gated verb, parsed from this very
/// line by `scripts/host-tools-doc-check.sh`. Keep it one line, no `"`.
pub const CONFIRM_SENTENCE: &str = "Calls affecting at least the plan's confirm_rows_min rows (host.quickstart limits.plan) or whole objects require `confirm` from a `dry_run: true` call with the same arguments (valid 10 min).";

/// The closed set of reasons a verb may opt out (R5).
pub const NO_DRY_RUN_REASONS: &[&str] = &["external_effect", "irreversible_by_design", "read_only_shape"];

const fn sv(name: &'static str, kind: Kind) -> WriteVerb {
    WriteVerb { name, kind, dry_run: DryRun::Supported }
}

const fn nd(name: &'static str, kind: Kind, reason: &'static str) -> WriteVerb {
    WriteVerb { name, kind, dry_run: DryRun::NoDryRun(reason) }
}

// One row per line, `sv("<verb>", Kind::X)` / `nd("<verb>", Kind::X, "<reason>")`:
// scripts/sandbox-api-doc-check.sh parses these lines to render docs/host-tools.md.
pub const WRITE_VERBS: &[WriteVerb] = &[
    sv("host.table.create", Kind::Write),
    sv("host.table.append", Kind::Write),
    sv("host.table.from_blob", Kind::Write),
    sv("host.table.drop", Kind::Drop),
    sv("host.table.handle_drop", Kind::Drop),
    sv("host.table.model_set", Kind::Write),
    sv("host.state.set", Kind::Write),
    sv("host.state.insert", Kind::Write),
    sv("host.state.delete", Kind::Delete),
    sv("host.state.delete_rows", Kind::Delete),
    sv("host.state.table_create", Kind::Write),
    sv("host.state.table_drop", Kind::Drop),
    sv("host.docs.put", Kind::Write),
    sv("host.docs.delete", Kind::Delete),
    sv("host.docs.purge", Kind::Delete),
    sv("host.index.create", Kind::Write),
    sv("host.index.delete", Kind::Delete),
    sv("host.enduser.purge", Kind::Delete),
    sv("host.enduser.revoke", Kind::Revoke),
    sv("host.enduser.unrevoke", Kind::Revoke),
    sv("host.oauth.revoke_all", Kind::Revoke),
    sv("host.oauth.grant_revoke", Kind::Revoke),
    sv("host.oauth.issuer_set", Kind::Write),
    sv("host.oauth.issuer_remove", Kind::Remove),
    sv("host.oauth.scope_set", Kind::Write),
    sv("host.oauth.policy_set", Kind::Write),
    sv("host.oauth.trusted_issuer_set", Kind::Write),
    sv("host.oauth.trusted_issuer_remove", Kind::Remove),
    sv("host.runs.purge", Kind::Delete),
    sv("host.tool.remove", Kind::Remove),
    sv("host.tool.publish", Kind::Publish),
    sv("host.trigger.set", Kind::Write),
    sv("host.trigger.pause", Kind::Write),
    sv("host.trigger.resume", Kind::Write),
    sv("host.trigger.remove", Kind::Remove),
    sv("host.group.create", Kind::Write),
    sv("host.group.remove", Kind::Remove),
    sv("host.invite.create", Kind::Write),
    sv("host.ask.create", Kind::Write),
    sv("host.invite.revoke", Kind::Revoke),
    sv("host.secret.set", Kind::Write),
    sv("host.policy.set", Kind::Write),
    sv("host.policy.attrs_set", Kind::Write),
    sv("host.agent.profile_set", Kind::Write),
    sv("host.share.caller_limit_remove", Kind::Remove),
    nd("host.key.rotate", Kind::Rotate, "external_effect"),
    nd("host.key.mint", Kind::Write, "external_effect"),
    nd("host.key.delegate", Kind::Write, "external_effect"),
    nd("host.key.revoke", Kind::Revoke, "irreversible_by_design"),
    nd("host.blob.put", Kind::Write, "external_effect"),
    nd("host.blob.delete", Kind::Delete, "external_effect"),
    nd("host.feedback.send", Kind::Write, "external_effect"),
    nd("host.example.propose", Kind::Write, "external_effect"),
    nd("host.self.offboard", Kind::Delete, "irreversible_by_design"),
    nd("host.enduser.assertion_secret_rotate", Kind::Rotate, "external_effect"),
    nd("host.registry.publish", Kind::Publish, "external_effect"),
    nd("host.index.refresh", Kind::Write, "external_effect"),
    nd("host.trigger.fire", Kind::Write, "external_effect"),
    nd("host.msg.send", Kind::Write, "external_effect"),
    nd("host.task.post", Kind::Write, "external_effect"),
    nd("host.channel.post", Kind::Write, "external_effect"),
    nd("host.uptime.create", Kind::Write, "external_effect"),
    nd("host.webhook_inbox.create", Kind::Write, "external_effect"),
    nd("host.vault.provider_set", Kind::Write, "external_effect"),
    nd("host.vault.provider_remove", Kind::Remove, "external_effect"),
    nd("host.oauth.provider_set", Kind::Write, "external_effect"),
    nd("host.oauth.provider_remove", Kind::Remove, "external_effect"),
];

const fn count_where(supported: bool) -> usize {
    let mut n = 0;
    let mut i = 0;
    while i < WRITE_VERBS.len() {
        if matches!(WRITE_VERBS[i].dry_run, DryRun::Supported) == supported {
            n += 1;
        }
        i += 1;
    }
    n
}

const SUPPORTED_COUNT: usize = count_where(true);
const OPT_OUT_COUNT: usize = count_where(false);

const fn supported_names() -> [&'static str; SUPPORTED_COUNT] {
    let mut out = [""; SUPPORTED_COUNT];
    let (mut i, mut j) = (0, 0);
    while i < WRITE_VERBS.len() {
        if matches!(WRITE_VERBS[i].dry_run, DryRun::Supported) {
            out[j] = WRITE_VERBS[i].name;
            j += 1;
        }
        i += 1;
    }
    out
}

const fn opt_outs() -> [(&'static str, &'static str); OPT_OUT_COUNT] {
    let mut out = [("", ""); OPT_OUT_COUNT];
    let (mut i, mut j) = (0, 0);
    while i < WRITE_VERBS.len() {
        if let DryRun::NoDryRun(reason) = WRITE_VERBS[i].dry_run {
            out[j] = (WRITE_VERBS[i].name, reason);
            j += 1;
        }
        i += 1;
    }
    out
}

/// Every verb that honours `dry_run: true` -- derived from [`WRITE_VERBS`].
pub const DRY_RUN_VERBS: [&str; SUPPORTED_COUNT] = supported_names();

/// Every opted-out verb with its reason -- derived from [`WRITE_VERBS`].
pub const NO_DRY_RUN_VERBS: [(&str, &str); OPT_OUT_COUNT] = opt_outs();

/// The registry row for `name`; a deprecated alias (`host.tool_remove`)
/// resolves to its canonical row first, so both share one.
pub fn lookup(name: &str) -> Option<&'static WriteVerb> {
    let canonical = crate::tool_aliases::resolve(name).unwrap_or(name);
    WRITE_VERBS.iter().find(|v| v.name == canonical)
}

/// R4: the `dry_run` property every `Supported` verb's `inputSchema` carries.
pub fn dry_run_property() -> Value {
    json!({
        "type": "boolean",
        "description": "return the diff this call would make and commit nothing",
    })
}

/// R4: adds `dry_run` to a `Supported` verb's schema properties (generated
/// from the registry; a verb whose literal descriptor already declares its
/// own `dry_run` -- `host.tool.publish`'s gate report -- keeps it).
pub fn splice_dry_run_property(name: &str, props: &mut Map<String, Value>) {
    if matches!(lookup(name), Some(WriteVerb { dry_run: DryRun::Supported, .. })) {
        props.entry("dry_run".to_string()).or_insert_with(dry_run_property);
    }
}

/// `true` iff `args.dry_run` is the boolean `true`.
pub fn dry_run_requested(args: &Value) -> bool {
    args.get("dry_run").and_then(Value::as_bool).unwrap_or(false)
}

/// The wire code of R5's refusal -- the same `args_invalid` every bad
/// argument gets, not a schema-validation site (no jsonschema error behind it).
const REFUSAL_CODE: &str = "args_invalid";

/// R5: the refusal a `NoDryRun` verb gives `dry_run: true`.
pub fn no_dry_run_error(verb: &WriteVerb, reason: &'static str) -> AppError {
    AppError::Structured {
        code: REFUSAL_CODE,
        message: format!("{} does not support dry_run ({reason})", verb.name),
        data: json!({"no_dry_run_reason": reason, "verb": verb.name}),
    }
}

/// R3: folds the notes a dry-run scope collected into
/// `{would: {...}, _meta: {dry_run: true}}`. A class nobody noted is
/// omitted; a verb that noted nothing specific still answers with one
/// `state` entry naming its target, so the caller always sees what it
/// aimed at.
pub fn build_would(verb: &WriteVerb, args: &Value, notes: &[(&'static str, Value)]) -> Value {
    let mut would = Map::new();
    for (class, item) in notes {
        would
            .entry((*class).to_string())
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .expect("class vectors are arrays")
            .push(item.clone());
    }
    if would.is_empty() {
        let target = ["name", "id", "table", "handle", "subject", "invite", "group", "key", "grant_id"]
            .iter()
            .find_map(|k| args.get(*k).and_then(Value::as_str))
            .unwrap_or("*");
        let action = format!("{:?}", verb.kind).to_lowercase();
        would.insert("state".to_string(), json!([{"key": format!("{}:{target}", verb.name), "action": action}]));
    }
    json!({"would": Value::Object(would), "_meta": {"dry_run": true}})
}
