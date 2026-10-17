//! PRD-mcphost-errors-json-contract: the one table of every error code the
//! host can return. `GET /errors.json`, `host.errors` and the `class` /
//! `family` / `remedy` / `help_url` fields `into_error_data` puts in every
//! error's `data` are all generated from [`ERROR_CODES`]; the
//! `errjson_ac01` scan test fails when a code appears in `src/` that is not
//! a row here, so the table cannot drift from the code.

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::doc_anchors::DocAnchor;
use crate::errors::AppError;

/// Whose fault an error is (the five-name set `/errors.json` lists under
/// `classes`, generated from these variants).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorClass {
    /// The caller sent something wrong; fix the call.
    AgentFault,
    /// The host declined by policy or state; the call was understood.
    HostRefusal,
    /// An upstream the tool depends on failed.
    UpstreamFault,
    /// A plan ceiling or rate was hit.
    PlanLimit,
    /// The host itself failed.
    HostFault,
}

impl ErrorClass {
    pub const ALL: &'static [ErrorClass] = &[
        ErrorClass::AgentFault,
        ErrorClass::HostRefusal,
        ErrorClass::UpstreamFault,
        ErrorClass::PlanLimit,
        ErrorClass::HostFault,
    ];

    /// The wire name; the exhaustive match keeps [`Self::ALL`] honest.
    pub const fn as_str(self) -> &'static str {
        match self {
            ErrorClass::AgentFault => "agent_fault",
            ErrorClass::HostRefusal => "host_refusal",
            ErrorClass::UpstreamFault => "upstream_fault",
            ErrorClass::PlanLimit => "plan_limit",
            ErrorClass::HostFault => "host_fault",
        }
    }
}

impl ErrorClass {
    /// One sentence for `/help/<code>` pages generated from the registry.
    pub const fn describe(self) -> &'static str {
        match self {
            ErrorClass::AgentFault => "The call itself was wrong; changing the call fixes it.",
            ErrorClass::HostRefusal => "The host understood the call and declined it by policy or state.",
            ErrorClass::UpstreamFault => "A service this tool depends on failed or refused the request.",
            ErrorClass::PlanLimit => "A plan ceiling or rate limit was reached.",
            ErrorClass::HostFault => "The host itself failed; it is not the caller's mistake.",
        }
    }
}

/// Which part of the product a code belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Family {
    Auth,
    Args,
    Spec,
    Publish,
    Call,
    Upstream,
    Plan,
    Compose,
    Storage,
    Internal,
    Oauth,
    Share,
    Registry,
}

impl Family {
    pub const fn as_str(self) -> &'static str {
        match self {
            Family::Auth => "auth",
            Family::Args => "args",
            Family::Spec => "spec",
            Family::Publish => "publish",
            Family::Call => "call",
            Family::Upstream => "upstream",
            Family::Plan => "plan",
            Family::Compose => "compose",
            Family::Storage => "storage",
            Family::Internal => "internal",
            Family::Oauth => "oauth",
            Family::Share => "share",
            Family::Registry => "registry",
        }
    }

    /// The doc section a family's codes are explained under (a
    /// compile-checked [`DocAnchor`], so it cannot name a missing heading).
    const fn docs_anchor(self) -> DocAnchor {
        match self {
            Family::Auth | Family::Oauth => DocAnchor::DocsAgentQuickstartQuickstartForAgents,
            Family::Args | Family::Publish | Family::Share | Family::Registry => {
                DocAnchor::DocsToolsTools
            }
            Family::Spec | Family::Compose => DocAnchor::ReadmeKinds,
            Family::Call => DocAnchor::ReadmeCallLimits,
            Family::Upstream => DocAnchor::ReadmeEgressProxyNetworkPublicEgress,
            Family::Plan => DocAnchor::DocsPlansPlanCeilings,
            Family::Storage | Family::Internal => DocAnchor::ReadmeGettingHelp,
        }
    }
}

/// One row of [`ERROR_CODES`].
#[derive(Debug, Clone, Copy)]
pub struct ErrorEntry {
    pub code: &'static str,
    pub family: Family,
    pub class: ErrorClass,
    /// Imperative, at most 200 chars; names the `data` field to read when one exists.
    pub remedy: &'static str,
    pub docs_anchor: DocAnchor,
    pub retryable: bool,
}

impl ErrorEntry {
    /// The seven-field JSON row (`help_url` is built on `base`).
    pub fn json_row(&self, base: &str) -> Value {
        json!({
            "code": self.code,
            "family": self.family.as_str(),
            "class": self.class.as_str(),
            "remedy": self.remedy,
            "docs_anchor": self.docs_anchor.as_str(),
            "retryable": self.retryable,
            "help_url": format!("{}/help/{}", base.trim_end_matches('/'), self.code),
        })
    }
}

const fn row(
    code: &'static str,
    family: Family,
    class: ErrorClass,
    retryable: bool,
    remedy: &'static str,
) -> ErrorEntry {
    ErrorEntry { code, family, class, remedy, docs_anchor: family.docs_anchor(), retryable }
}

/// The registry row for `code`, if any.
pub fn lookup(code: &str) -> Option<&'static ErrorEntry> {
    ERROR_CODES.iter().find(|e| e.code == code)
}

/// The `/errors.json` / `host.errors` object, built from [`ERROR_CODES`]
/// (sorted by code) on `base`, this request's own origin.
pub fn catalog_json(base: &str) -> Value {
    let mut rows: Vec<&ErrorEntry> = ERROR_CODES.iter().collect();
    rows.sort_by_key(|e| e.code);
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "classes": ErrorClass::ALL.iter().map(|c| c.as_str()).collect::<Vec<_>>(),
        "codes": rows.iter().map(|e| e.json_row(base)).collect::<Vec<_>>(),
    })
}

/// `host.errors`'s description (it is the tool-list text an agent reads).
pub const TOOL_DESCRIPTION: &str = "List every error code this host can return, each with its class \
    (agent_fault | host_refusal | upstream_fault | plan_limit | host_fault), family, remedy, docs \
    anchor, retryable flag and help_url -- the same object GET /errors.json serves. Pass code to get \
    one row; a misspelt code answers tool_not_found with did_you_mean. Read-only, anonymous, no \
    tenant data.";

/// `host.errors`'s input properties (`host_schema` adds `tenant_key`).
pub fn schema_properties() -> Value {
    json!({
        "code": {
            "type": "string",
            "description": "An error code, e.g. tool_exception. Omit to list every code.",
        },
    })
}

/// The codes nearest `requested` by edit distance (at most three, within
/// two edits), nearest first.
fn nearest_codes(requested: &str) -> Vec<&'static str> {
    let lower = requested.to_ascii_lowercase();
    let mut scored: Vec<(usize, &'static str)> = ERROR_CODES
        .iter()
        .map(|e| (crate::errors::levenshtein(&lower, e.code), e.code))
        .filter(|(d, _)| *d <= 2)
        .collect();
    scored.sort();
    scored.into_iter().take(3).map(|(_, c)| c).collect()
}

/// `host.errors {code?}`: with no `code` the [`catalog_json`] object (what
/// `/errors.json` serves); with one, that row. An unknown `code` is a
/// `tool_not_found`-shaped error naming the nearest code in `did_you_mean`.
pub fn call(base: &str, args: &Value) -> Result<Value, AppError> {
    let code = match args.get("code") {
        None | Some(Value::Null) => return Ok(catalog_json(base)),
        Some(Value::String(code)) => code,
        Some(_) => return Err(AppError::InvalidArgs("code: expected a string".to_string())),
    };
    if let Some(row) = lookup(code) {
        return Ok(row.json_row(base));
    }
    let nearest = nearest_codes(code);
    let mut data = Map::new();
    data.insert("error".to_string(), json!("unknown_error_code"));
    data.insert("name".to_string(), json!(code));
    data.insert("nearest".to_string(), json!(nearest));
    if let Some(best) = nearest.first() {
        data.insert("did_you_mean".to_string(), json!(best));
    }
    data.insert("hint".to_string(), json!("host.errors with no code lists every code"));
    Err(AppError::Structured {
        code: "tool_not_found",
        message: format!("unknown error code {code}; nearest: {}", nearest.join(", ")),
        data: Value::Object(data),
    })
}

/// Every code the host can return, sorted by code.
pub const ERROR_CODES: &[ErrorEntry] = &[
    row("agent_not_found", Family::Share, ErrorClass::AgentFault, false, "Check the address with host.agent.lookup, then re-send to a handle that exists."),
    row("alert_body_too_large", Family::Args, ErrorClass::AgentFault, false, "Shorten the alert body to data.limit_bytes or less and re-send."),
    row("anchor_not_found", Family::Args, ErrorClass::AgentFault, false, "Pick one of data.did_you_mean, or call host.help.search for a real anchor."),
    row("archive_corrupt", Family::Storage, ErrorClass::AgentFault, false, "Re-run host.export and import the fresh archive; this one is damaged or not an export."),
    row("args_invalid", Family::Args, ErrorClass::AgentFault, false, "Re-send with data.field as data.expected; data.example shows a valid value."),
    row("args_schema_not_inferable", Family::Spec, ErrorClass::AgentFault, false, "Declare args_schema explicitly in the spec; the source's argument use cannot be inferred."),
    row("ask_answered", Family::Share, ErrorClass::AgentFault, false, "Read the recorded answer in the response body; the first answer wins and cannot be changed."),
    row("ask_expired", Family::Share, ErrorClass::AgentFault, false, "Do not act on a missing answer; create a new ask with host.ask.create."),
    row("ask_not_found", Family::Share, ErrorClass::AgentFault, false, "List asks with host.ask.list and use an id from data.next_call's result."),
    row("asks_limit", Family::Plan, ErrorClass::PlanLimit, false, "Wait for a pending ask to be answered or expire, or reuse one from host.ask.list; data.asks_open_max is the ceiling."),
    row("auth_conflict", Family::Auth, ErrorClass::AgentFault, false, "Send exactly one credential: either the URL secret or the Authorization header, not two different tenants."),
    row("authorization_required", Family::Oauth, ErrorClass::HostRefusal, false, "Authorize through the OAuth metadata named in the WWW-Authenticate header, then retry with the token."),
    row("banned", Family::Auth, ErrorClass::HostRefusal, false, "This key is banned; do not retry. Contact support through the support_url on /support.html."),
    row("bearer_invalid", Family::Auth, ErrorClass::AgentFault, false, "Call signup for a key, then send Authorization: Bearer <key> or pass tenant_key as an argument."),
    row("billing_unavailable", Family::Plan, ErrorClass::HostRefusal, false, "Billing is not configured on this host; do not retry. Read billing.plans for what the host offers."),
    row("blob_expired", Family::Storage, ErrorClass::AgentFault, false, "Revive it with host.blob.touch {id, ttl_s} before the nightly prune, or upload the blob again."),
    row("blob_not_found", Family::Storage, ErrorClass::AgentFault, false, "List blobs with host.blob.list and use an id or name that exists for this tenant."),
    row("blob_too_large", Family::Plan, ErrorClass::PlanLimit, false, "The blob is larger than blob_max_bytes (data.limit); upload a smaller file or move to a plan with a larger blob_max_bytes."),
    row("blob_too_large_inline", Family::Args, ErrorClass::AgentFault, false, "Fetch the blob through the link from host.blob.url instead of reading it inline."),
    row("budget_ceiling_exceeded", Family::Plan, ErrorClass::PlanLimit, false, "Lower the requested budget value to data.ceiling or below, or move to a plan with a higher ceiling."),
    row("budget_exceeded", Family::Call, ErrorClass::PlanLimit, false, "Raise the run budget on data.dimension within the plan ceiling, or reduce the work per call."),
    row("build_failed", Family::Call, ErrorClass::AgentFault, false, "Read data.tail for the build error, fix the requirements in the spec, and republish."),
    row("call_timeout", Family::Call, ErrorClass::AgentFault, true, "Reduce the work per call or raise timeout_s within the plan ceiling, then retry."),
    row("capability_unsupported", Family::Spec, ErrorClass::HostRefusal, false, "Do not retry; use data.alternative when present or read /capabilities.json for implemented primitives."),
    row("capacity", Family::Call, ErrorClass::HostFault, true, "Retry after data.retry_after_ms; the host is at its concurrent-call limit."),
    row("channel_closed", Family::Share, ErrorClass::AgentFault, false, "Posting is refused on a closed channel; reads still work. Open a new channel to continue."),
    row("channel_frozen", Family::Share, ErrorClass::AgentFault, false, "Posting is refused on a frozen channel; reads still work. Ask the owner to unfreeze it."),
    row("channel_not_found", Family::Share, ErrorClass::AgentFault, false, "Check data.key against host.channel.list and use a channel id or name that exists."),
    row("channel_unavailable", Family::Internal, ErrorClass::HostRefusal, false, "This call context has no channel backend; call the tool directly instead of from a sandbox."),
    row("chart_invalid_mark", Family::Args, ErrorClass::AgentFault, false, "Re-send with one of data.allowed as the mark."),
    row("choice_invalid", Family::Share, ErrorClass::AgentFault, false, "Choose one of the ask's listed options and answer again."),
    row("claim_email_failed", Family::Internal, ErrorClass::HostFault, true, "The claim email could not be sent; retry later or use the claim link from the response."),
    row("client_not_found", Family::Oauth, ErrorClass::AgentFault, false, "Check data.client_id against the pending clients list and use one that exists."),
    row("compose_children_exceeded", Family::Compose, ErrorClass::AgentFault, false, "Reduce the number of child calls in the compose spec to the plan ceiling."),
    row("compose_depth_exceeded", Family::Compose, ErrorClass::AgentFault, false, "Flatten the compose spec; nesting is deeper than the allowed depth."),
    row("compose_fanout_exceeded", Family::Compose, ErrorClass::AgentFault, false, "Reduce the for_each fan-out to the allowed number of items."),
    row("compose_fanout_item_failed", Family::Compose, ErrorClass::UpstreamFault, false, "Read data for the failing item's own error and fix or skip that item."),
    row("compose_input_missing", Family::Compose, ErrorClass::AgentFault, false, "Pass the input the compose mapping reads, named in the message."),
    row("compose_item_outside_for_each", Family::Compose, ErrorClass::AgentFault, false, "Use item only inside a for_each mapping."),
    row("compose_mapping_missing", Family::Compose, ErrorClass::AgentFault, false, "Add the missing mapping for the child argument named in the message."),
    row("compose_mapping_type", Family::Compose, ErrorClass::AgentFault, false, "Map a value of the child argument's declared type."),
    row("compose_mapping_unresolved", Family::Compose, ErrorClass::AgentFault, false, "Fix the mapping path named in data.field so it resolves against the inputs."),
    row("compose_self_call", Family::Compose, ErrorClass::AgentFault, false, "A compose tool may not call itself; point the step at a different tool."),
    row("concurrency_exceeded", Family::Plan, ErrorClass::PlanLimit, true, "Wait for in-flight calls to finish and retry; data.plan names the concurrency ceiling."),
    row("confirm_invalid", Family::Call, ErrorClass::AgentFault, false, "Call the dry run again and repeat with the freshly returned confirm token."),
    row("confirm_required", Family::Call, ErrorClass::AgentFault, false, "Call dry_run_first.tool with dry_run_first.args, read would, then repeat with the returned confirm token."),
    row("conflicting_credentials", Family::Auth, ErrorClass::AgentFault, false, "Send one credential: drop the tenant_key argument or the Authorization header."),
    row("contact_pending", Family::Share, ErrorClass::AgentFault, false, "A request is already pending or recently denied; wait for the recipient to answer."),
    row("contact_refused", Family::Share, ErrorClass::HostRefusal, false, "Send host.agent.contact_request first and wait for acceptance."),
    row("contact_request_not_found", Family::Share, ErrorClass::AgentFault, false, "List pending requests and use an id that exists."),
    row("cursor_invalid", Family::Args, ErrorClass::AgentFault, false, crate::paged::CURSOR_REMEDY),
    row("dead_letter_not_found", Family::Storage, ErrorClass::AgentFault, false, "List dead letters and use an id that exists for this tenant."),
    row("dependency_advisory", Family::Spec, ErrorClass::HostRefusal, false, "Pin the package to data.fixed or later and republish."),
    row("dependency_policy", Family::Spec, ErrorClass::HostRefusal, false, "Remove or replace the requirement named in data.reason; the policy denies it."),
    row("dependency_resolve_failed", Family::Spec, ErrorClass::AgentFault, false, "Read data.tail for the resolver error and fix the requirements."),
    row("docs_mime_unsupported", Family::Args, ErrorClass::AgentFault, false, "Use one of data.allowed mimes; binary files go to host.blob.put."),
    row("docs_not_found", Family::Storage, ErrorClass::AgentFault, false, "List documents with host.docs.list and use a name or id that exists."),
    row("docs_provider_error", Family::Upstream, ErrorClass::UpstreamFault, true, "The embeddings provider returned an error status; retry later or check the provider config."),
    row("docs_provider_unreachable", Family::Upstream, ErrorClass::UpstreamFault, true, "The embeddings provider could not be reached; retry later or check its URL."),
    row("docs_secret_missing", Family::Args, ErrorClass::AgentFault, false, "Set the secret named in data.secret with host.secret_set, then retry."),
    row("docs_too_large", Family::Plan, ErrorClass::PlanLimit, false, "Shorten the document to data.limit_bytes or split it."),
    row("docs_unavailable", Family::Internal, ErrorClass::HostRefusal, false, "This call context has no document backend; call the tool directly instead of from a sandbox."),
    row("draft_test_failed", Family::Call, ErrorClass::AgentFault, false, "Read data for the failing draft test and adjust the request."),
    row("draft_unavailable", Family::Internal, ErrorClass::HostRefusal, false, "This host has no draft model configured; build the spec by hand from host.quickstart."),
    row("drift_review_not_found", Family::Storage, ErrorClass::AgentFault, false, "List reviews with host.drift.list and use an item id that exists."),
    row("drift_target_not_found", Family::Storage, ErrorClass::AgentFault, false, "Declare the table or document first, or use a target that already exists."),
    row("drift_unavailable", Family::Internal, ErrorClass::HostRefusal, false, "This call context has no drift backend; call the tool directly instead of from a sandbox."),
    row("egress_unavailable", Family::Internal, ErrorClass::HostRefusal, false, "Network egress is not configured on this host; use a kind or host that does not need it."),
    row("elicit_calls_exceeded", Family::Call, ErrorClass::AgentFault, false, "Ask for fewer elicitations per call; data.limit is the maximum."),
    row("elicit_schema_invalid", Family::Args, ErrorClass::AgentFault, false, "Use only the schema subset in data.allowed and stay within data.limits."),
    row("elicit_timeout", Family::Call, ErrorClass::AgentFault, true, "The client did not answer in time; call again and answer within data.timeout_s."),
    row("elicit_unsupported", Family::Call, ErrorClass::HostRefusal, false, "This client or context cannot elicit; read data.reason and ask through host.ask.create instead."),
    row("email_not_owner", Family::Args, ErrorClass::AgentFault, false, "Set target to the owner's verified email address, or add a webhook or slack channel for another destination."),
    row("end_user_assertion_invalid", Family::Auth, ErrorClass::AgentFault, false, "Send a fresh end_user_assertion; data.reason names what was wrong."),
    row("end_user_explicit_forbidden", Family::Auth, ErrorClass::AgentFault, false, "Drop the explicit end_user subject; this call already carries an end-user identity."),
    row("end_user_not_found", Family::Storage, ErrorClass::AgentFault, false, "List end users and use a subject that exists for this tenant."),
    row("end_user_required", Family::Auth, ErrorClass::AgentFault, false, "Connect with a verified end-user identity, or pass an explicit end_user subject."),
    row("end_user_revoked", Family::Auth, ErrorClass::HostRefusal, false, "This end user was revoked; do not retry with that subject."),
    row("env_secret_collision", Family::Spec, ErrorClass::AgentFault, false, "Rename the env entry or the secret so no name is used by both."),
    row("event_body_too_large", Family::Plan, ErrorClass::PlanLimit, false, "Send a smaller event body; the limit is the plan's event_body_bytes_max."),
    row("events_rate_limited", Family::Plan, ErrorClass::PlanLimit, true, "Slow the delivery rate and retry; the plan caps events_per_minute."),
    row("example_exists", Family::Share, ErrorClass::AgentFault, false, "Rename the tool and propose again."),
    row("example_hidden", Family::Share, ErrorClass::HostRefusal, true, "The example failed its nightly proof and is hidden (data.reason, data.red_since); pick another from host.example.list or retry after it is green again."),
    row("example_not_found", Family::Share, ErrorClass::AgentFault, false, "Call host.example.list for the example names and pass one of them as name."),
    row("example_unavailable", Family::Spec, ErrorClass::AgentFault, false, "Pass args explicitly; no example could be generated from args_schema."),
    row("exec_unavailable", Family::Internal, ErrorClass::HostFault, true, "The sandbox or exec backend is down; retry later or use a kind that does not need it."),
    row("export_link_expired", Family::Storage, ErrorClass::AgentFault, false, "Run host.export again to get a fresh signed link."),
    row("export_not_complete", Family::Storage, ErrorClass::AgentFault, true, "Wait for the export run to complete, then import."),
    row("export_too_large", Family::Plan, ErrorClass::PlanLimit, false, "Reduce the tenant's data below the plan's export_bytes_max, or move to a larger plan."),
    row("feedback_not_found", Family::Storage, ErrorClass::AgentFault, false, "Use a feedback id this caller filed."),
    row("field_unknown", Family::Args, ErrorClass::AgentFault, false, "Use a field from data.valid; data.nearest is the closest match."),
    row("fixture_shape_mismatch", Family::Spec, ErrorClass::AgentFault, false, "Give the fixture one of the declared outputs named in data.expected."),
    row("fixture_step_ambiguous", Family::Spec, ErrorClass::AgentFault, false, "Key the fixture by step number; several steps use that tool."),
    row("fixture_step_unknown", Family::Spec, ErrorClass::AgentFault, false, "Key the fixture by a step that exists; data.steps is the step count."),
    row("fixtures_not_applicable", Family::Spec, ErrorClass::AgentFault, false, "Fixtures apply to chain tools only; drop them for other kinds."),
    row("fixtures_too_large", Family::Plan, ErrorClass::PlanLimit, false, "Reduce the fixtures to data.limit_bytes in total."),
    row("forbidden", Family::Auth, ErrorClass::HostRefusal, false, "This credential may not do that; use a key with the needed access."),
    row("grant_not_found", Family::Oauth, ErrorClass::AgentFault, false, "List grants and use an id that is still live for this tenant."),
    row("group_not_found", Family::Share, ErrorClass::AgentFault, false, "List groups and use an id that exists."),
    row("handle_not_found", Family::Storage, ErrorClass::AgentFault, false, "List handles and use one that exists for this tenant."),
    row("handle_quota_exceeded", Family::Plan, ErrorClass::PlanLimit, false, "Drop unused handles or move to a plan with a larger handle byte quota."),
    row("handle_reserved", Family::Share, ErrorClass::AgentFault, false, "Choose a different handle; this one is reserved."),
    row("handle_taken", Family::Share, ErrorClass::AgentFault, false, "Choose a different handle; this one is already claimed."),
    row("handoff_token_expired", Family::Auth, ErrorClass::AgentFault, false, "Request a new handoff token and redeem it before it expires."),
    row("handoff_token_invalid", Family::Auth, ErrorClass::AgentFault, false, "Request a new handoff token; this one is not valid."),
    row("handoff_token_redeemed", Family::Auth, ErrorClass::AgentFault, false, "Request a new handoff token; each token redeems once."),
    row("has_children", Family::Auth, ErrorClass::AgentFault, false, "Pass cascade: true to revoke the sub-key's live descendants too (data.count), or revoke them first."),
    row("hook_not_found", Family::Storage, ErrorClass::AgentFault, false, "Create the event trigger for this tool first, or use one that exists."),
    row("host_not_allowed", Family::Call, ErrorClass::HostRefusal, false, "Call a public host; private, loopback, link-local and internal hosts are refused."),
    row("host_unavailable", Family::Upstream, ErrorClass::UpstreamFault, true, "The host did not answer; retry later or check /healthz."),
    row("host_unreachable", Family::Upstream, ErrorClass::UpstreamFault, true, "The host could not be reached from this client; check the URL and network, then retry."),
    row("index_exists", Family::Storage, ErrorClass::AgentFault, false, "Use a different index name or drop the existing index first."),
    row("index_not_found", Family::Storage, ErrorClass::AgentFault, false, "Create the index first or use a name from the index list."),
    row("insufficient_scope", Family::Auth, ErrorClass::HostRefusal, false, "Use a token that carries the scope named in data.scope."),
    row("internal", Family::Internal, ErrorClass::HostFault, true, "Retry once; if it persists, report data.request_id through the support link."),
    row("interrupted", Family::Internal, ErrorClass::HostFault, true, "The server restarted before this call finished; retry the same call (data.restart_id names the restart)."),
    row("invalid_example", Family::Share, ErrorClass::AgentFault, false, "Fix the errors listed in data.errors and propose again."),
    row("invalid_params", Family::Args, ErrorClass::AgentFault, false, "Re-send with the params the method expects; data names the limit or field."),
    row("invalid_spec", Family::Spec, ErrorClass::AgentFault, false, "Fix the spec field named in data.field to match data.expected."),
    row("invalid_token", Family::Auth, ErrorClass::AgentFault, false, "Fetch a new token; data.error_description says whether it was expired, malformed or from an unknown issuer."),
    row("invalid_tool_name", Family::Args, ErrorClass::AgentFault, false, "Use a tool name of lowercase letters, digits and underscores within the length limit."),
    row("invalid_webhook_signature", Family::Auth, ErrorClass::AgentFault, false, "Re-send with a signature computed over the raw body with the signing secret."),
    row("invite_invalid", Family::Auth, ErrorClass::HostRefusal, false, "Ask the owner for a new invite link; this one is no longer valid."),
    row("invite_rate_limited", Family::Plan, ErrorClass::HostRefusal, true, "Try the invite link again later; it reached its hourly join limit."),
    row("invites_quota_exceeded", Family::Plan, ErrorClass::PlanLimit, false, "Revoke an unused invite or move to a plan with a higher invites_max."),
    row("issuer_already_registered", Family::Oauth, ErrorClass::AgentFault, false, "Use the existing issuer registration or remove it first."),
    row("issuer_not_found", Family::Oauth, ErrorClass::AgentFault, false, "List issuers and use a registered one."),
    row("issuer_quota_exceeded", Family::Plan, ErrorClass::PlanLimit, false, "Remove an unused issuer; data.limit is the plan's maximum."),
    row("issuer_url_invalid", Family::Oauth, ErrorClass::AgentFault, false, "Use an https:// issuer URL."),
    row("jobs_concurrent", Family::Plan, ErrorClass::PlanLimit, true, "Wait for a running job to finish; the tenant is at its jobs_concurrent ceiling."),
    row("jobs_concurrent_exceeded", Family::Plan, ErrorClass::PlanLimit, true, "Wait for a running job to finish; the tenant is at its jobs_concurrent ceiling."),
    row("join_code_invalid", Family::Auth, ErrorClass::HostRefusal, false, "Ask a client already in the workspace for a fresh join link; host.whoami (data.next_call) prints the live join_url."),
    row("jsonrpc_error", Family::Call, ErrorClass::AgentFault, false, "Read the JSON-RPC error's own code and message returned by the host."),
    row("key_bound_elsewhere", Family::Auth, ErrorClass::AgentFault, false, "Use this sub-key only from the session or run it is bound to (data.bound), or delegate a new sub-key with host.key.delegate."),
    row("key_expired", Family::Auth, ErrorClass::AgentFault, false, "Create a new sub-key with host.key.create."),
    row("key_revoked", Family::Auth, ErrorClass::AgentFault, false, "Create a new sub-key; this one was revoked."),
    row("kind_mismatch", Family::Spec, ErrorClass::AgentFault, false, "Send the kind the spec's shape needs (data.inferred), or change the spec to match data.requested."),
    row("label_taken", Family::Auth, ErrorClass::AgentFault, false, "Choose a different sub-key label."),
    row("last_client", Family::Auth, ErrorClass::AgentFault, false, "Add another client to the workspace first (host.whoami prints the join_url), then revoke this one."),
    row("lineage_blocked", Family::Storage, ErrorClass::AgentFault, false, "Repeat with confirm: true, or remove the consumers named in data.lineage_blocked first."),
    row("lineage_handle_not_found", Family::Storage, ErrorClass::AgentFault, false, "Run the trace again and use the fresh handle."),
    row("lineage_unavailable", Family::Internal, ErrorClass::HostRefusal, false, "This call context has no lineage backend; call the tool directly instead of from a sandbox."),
    row("mesh_frozen", Family::Share, ErrorClass::HostRefusal, false, "Messaging and posting are paused by the operator for this tenant; do not retry."),
    row("meter_emit_locked", Family::Internal, ErrorClass::HostFault, true, "Another emit-meter run holds the lock; retry when it finishes."),
    row("msg_unavailable", Family::Internal, ErrorClass::HostRefusal, false, "This call context has no messaging backend; call the tool directly instead of from a sandbox."),
    row("namespace_unverified", Family::Registry, ErrorClass::HostRefusal, false, "Verify the namespace with the registry before publishing."),
    row("no_channel", Family::Args, ErrorClass::AgentFault, false, "Register a channel with host.notify.channel_add, then send the notify again."),
    row("not_a_member", Family::Share, ErrorClass::AgentFault, false, "Call host.org_members for the live member list; this namespace is not one of them."),
    row("not_found", Family::Storage, ErrorClass::AgentFault, false, "Use a run_id or url from a completed host.export."),
    row("not_in_org", Family::Share, ErrorClass::AgentFault, false, "Call host.org_create or host.org_join first; this tenant belongs to no org."),
    row("not_needed", Family::Share, ErrorClass::AgentFault, false, "Send the message directly; no contact request is needed."),
    row("not_owner", Family::Share, ErrorClass::HostRefusal, false, "Only the org owner can do this; ask the owner listed by host.org_members."),
    row("not_portable", Family::Share, ErrorClass::AgentFault, false, "Remove the reference to your tenant named in data.reference and propose again."),
    row("notify_channel_limit", Family::Plan, ErrorClass::PlanLimit, false, "Remove a channel with host.notify.channel_remove or move to a plan with a higher notify_channels_max."),
    row("notify_channel_not_found", Family::Args, ErrorClass::AgentFault, false, "List channels with host.notify.channel_list and use a channel_id that exists on this tenant."),
    row("notify_kind_unknown", Family::Args, ErrorClass::AgentFault, false, "Call host.notify.channel_add with a supported kind; data.kinds lists the kinds this host delivers to."),
    row("notify_limit", Family::Plan, ErrorClass::PlanLimit, true, "The daily notify quota is used; retry after data.resets_unix or move to a plan with a higher notifies_per_day."),
    row("notify_target_invalid", Family::Args, ErrorClass::AgentFault, false, "Fix target for the channel kind; a webhook or slack target must be an https URL on a public host."),
    row("org_member_already", Family::Share, ErrorClass::AgentFault, false, "This tenant is already in an org; call host.org_members to see it."),
    row("org_members_max", Family::Plan, ErrorClass::PlanLimit, false, "Upgrade to the plan named in data.upgrade, or remove a member first."),
    row("org_name_taken", Family::Share, ErrorClass::AgentFault, false, "Choose a different org name; this one is already taken."),
    row("owner_cannot_leave", Family::Share, ErrorClass::AgentFault, false, "The owner cannot be removed from its own org; remove other members or delete the org instead."),
    row("owner_unverified", Family::Auth, ErrorClass::HostRefusal, false, "Verify the owner email first (data.next_call is host.claim), or add a webhook or slack channel instead."),
    row("permission_denied", Family::Auth, ErrorClass::HostRefusal, false, "Use a key allowed to call this tool; data.allowed_tools lists what this sub-key may call."),
    row("plan_limit", Family::Plan, ErrorClass::PlanLimit, false, "Wait for the daily limit to reset or move to a plan with a higher limit."),
    row("plan_required", Family::Plan, ErrorClass::PlanLimit, false, "Move to a plan that includes this feature; data names it."),
    row("policy_widening", Family::Spec, ErrorClass::AgentFault, false, "Repeat with replace: true to confirm the wider rule, or narrow the proposed rule."),
    row("proof_failed", Family::Share, ErrorClass::AgentFault, false, "Fix the tool so proof_call holds; data.actual is the result it returned."),
    row("provider_discovery_failed", Family::Oauth, ErrorClass::UpstreamFault, true, "Check the issuer URL serves a discovery document, then retry."),
    row("provider_not_found", Family::Oauth, ErrorClass::AgentFault, false, "Register an OIDC provider first."),
    row("quota_blob_bytes", Family::Plan, ErrorClass::PlanLimit, false, "Delete blobs or move to a plan with a higher blob_bytes_max; data.limit is the plan's maximum."),
    row("quota_caller", Family::Plan, ErrorClass::PlanLimit, true, "The caller exceeded its calls_per_day limit for this shared tool; retry after the daily reset."),
    row("quota_docs", Family::Plan, ErrorClass::PlanLimit, false, "Delete a document or move to a plan with a higher docs_max."),
    row("quota_docs_bytes", Family::Plan, ErrorClass::PlanLimit, false, "Delete documents or move to a plan with a higher docs_bytes_max."),
    row("quota_end_users", Family::Plan, ErrorClass::PlanLimit, false, "Revoke unused end users or move to a plan with a higher end_users_max."),
    row("quota_exceeded", Family::Plan, ErrorClass::PlanLimit, false, "Read data for the exceeded quota; reduce usage or move to a plan with a higher limit."),
    row("quota_index_max", Family::Plan, ErrorClass::PlanLimit, false, "Drop an unused index or move to a plan with a higher index_max."),
    row("quota_trusted_issuers", Family::Plan, ErrorClass::PlanLimit, false, "Remove a trusted issuer; data.limit is the plan's maximum."),
    row("quota_vault_providers", Family::Plan, ErrorClass::PlanLimit, false, "Remove an unused vault provider or move to a plan with a higher vault_providers_max."),
    row("rate_limited", Family::Plan, ErrorClass::PlanLimit, true, "Wait data.retry_after_s seconds and retry."),
    row("rate_limited_host", Family::Internal, ErrorClass::HostFault, true, "The host is shedding load; retry shortly."),
    row("recipient_inbox_full", Family::Share, ErrorClass::HostRefusal, true, "Try again later; that agent's inbox is full."),
    row("registry_disabled", Family::Registry, ErrorClass::HostRefusal, false, "This host has no registry enabled; do not retry."),
    row("registry_rejected", Family::Registry, ErrorClass::UpstreamFault, false, "The registry refused the publish; correct the spec and retry."),
    row("remember_too_long", Family::Args, ErrorClass::AgentFault, false, "Shorten remember to 4096 bytes or fewer."),
    row("replay_needs_secret", Family::Call, ErrorClass::AgentFault, false, "Pass the full replay.args; the recorded request was too large to keep."),
    row("requirement_not_allowed", Family::Spec, ErrorClass::HostRefusal, false, "Use only requirements allowed by the PyPI policy."),
    row("requirement_not_inferable", Family::Spec, ErrorClass::AgentFault, false, "List requirements explicitly in the spec; the import cannot be mapped."),
    row("resources_max", Family::Plan, ErrorClass::PlanLimit, false, "Unsubscribe from a resource or move to a plan with a higher resources_max; data.limit is the plan maximum of open resource subscriptions."),
    row("response_too_large", Family::Upstream, ErrorClass::UpstreamFault, false, "Ask the upstream for a smaller response; the cap is in the message."),
    row("revoke_required", Family::Auth, ErrorClass::AgentFault, false, "Revoke the end user first, then purge."),
    row("run_error", Family::Call, ErrorClass::AgentFault, false, "Read the run's error_class and fix the tool."),
    row("run_not_cancellable", Family::Call, ErrorClass::AgentFault, false, "The run is already finished or unknown; nothing to cancel."),
    row("run_not_found", Family::Storage, ErrorClass::AgentFault, false, "List runs and use an id that exists for this tenant."),
    row("sandbox_unavailable", Family::Internal, ErrorClass::HostFault, true, "Retry in data.retry_after_s seconds or use http instead; the python kind is unavailable."),
    row("sandbox_unsupported", Family::Internal, ErrorClass::HostRefusal, false, "No registered kind on this host has a sandbox self-test; do not retry."),
    row("scope_escalation", Family::Auth, ErrorClass::AgentFault, false, "Request only scopes, tools and limits inside the delegating key's own authority; data.excess names the one over."),
    row("secret_leak", Family::Share, ErrorClass::AgentFault, false, "Replace the literal value with the secret placeholder named in the message and republish."),
    row("secret_missing", Family::Call, ErrorClass::AgentFault, false, "Set the missing secret with host.secret_set, then retry."),
    row("secret_unavailable", Family::Call, ErrorClass::AgentFault, false, "Set the missing secret with host.secret_set, then retry."),
    row("service_unavailable", Family::Internal, ErrorClass::HostFault, true, "Retry later; the host is out of a resource such as disk space."),
    row("share_quota_exceeded", Family::Plan, ErrorClass::PlanLimit, false, "Unshare a tool or move to a plan with a higher share limit."),
    row("signature_invalid", Family::Auth, ErrorClass::AgentFault, false, "Re-send with a valid signature, token and timestamp for the verify config."),
    row("signature_mismatch", Family::Auth, ErrorClass::AgentFault, false, "Re-send with a signature that matches the verify config."),
    row("signup_paused", Family::Auth, ErrorClass::HostRefusal, true, "Retry after data.retry_after_secs; signup is paused."),
    row("signup_rate_limited", Family::Plan, ErrorClass::HostRefusal, true, "Retry after data.retry_after_s; too many tenants were created from this address."),
    row("spec_not_exposed", Family::Share, ErrorClass::HostRefusal, false, "Ask the owner to expose the spec; it is not shared."),
    row("spec_too_large", Family::Plan, ErrorClass::PlanLimit, false, "Reduce the spec below the size cap, for example by moving data to host.blob.put."),
    row("state_quota", Family::Plan, ErrorClass::PlanLimit, false, "Free state space or move to a plan with a larger state quota."),
    row("state_quota_exceeded", Family::Plan, ErrorClass::PlanLimit, false, "Delete state rows or move to a plan with a higher state_bytes_max."),
    row("state_table_not_found", Family::Storage, ErrorClass::AgentFault, false, "Declare the table first or use a declared name."),
    row("state_unavailable", Family::Internal, ErrorClass::HostRefusal, false, "This call context has no state backend; call the tool directly instead of from a sandbox."),
    row("step_tool_not_allowed", Family::Spec, ErrorClass::AgentFault, false, "Use a host step from the allowlist named in the message."),
    row("step_tool_not_found", Family::Spec, ErrorClass::AgentFault, false, "Use a tool that exists; data.suggestions lists near matches."),
    row("storage", Family::Storage, ErrorClass::HostFault, true, "Retry once; if it persists, report data.request_id through the support link."),
    row("table_already_exists", Family::Storage, ErrorClass::AgentFault, false, "Use a different table name or drop the existing one."),
    row("table_bound_exceeded", Family::Call, ErrorClass::AgentFault, false, "Narrow the query so it stays under data.bound's limit."),
    row("table_not_found", Family::Storage, ErrorClass::AgentFault, false, "Declare the table first or use a declared name."),
    row("table_query_rejected", Family::Args, ErrorClass::AgentFault, false, "Fix the query; data.reason says why it was rejected."),
    row("table_unavailable", Family::Internal, ErrorClass::HostRefusal, false, "This call context has no table backend; call the tool directly instead of from a sandbox."),
    row("task_already_claimed", Family::Share, ErrorClass::AgentFault, false, "Pick another task; this one is already claimed."),
    row("task_closed", Family::Share, ErrorClass::AgentFault, false, "The task takes no further claim or completion; post a new task."),
    row("task_not_claimed", Family::Share, ErrorClass::AgentFault, false, "Claim the task with host.task.claim before completing it."),
    row("task_not_found", Family::Share, ErrorClass::AgentFault, false, "List tasks and use an id that exists."),
    row("template_error", Family::Spec, ErrorClass::AgentFault, false, "Fix the template in the field named in data.field."),
    row("tenant_disabled", Family::Auth, ErrorClass::HostRefusal, false, "This tenant is disabled; read data.disabled_reason and do not retry."),
    row("tenant_key_invalid", Family::Auth, ErrorClass::AgentFault, false, "Call signup for a fresh key; this tenant_key matched no tenant."),
    row("tenant_key_missing", Family::Auth, ErrorClass::AgentFault, false, "Pass tenant_key, or reconnect through your onboarding url; data.tenant names the tenant when known."),
    row("tenant_key_required", Family::Auth, ErrorClass::HostRefusal, false, "Call with the tenant key only; this tool refuses an end-user identity."),
    row("tenant_not_empty", Family::Storage, ErrorClass::AgentFault, false, "Import into a new empty tenant; this one already holds data."),
    row("tenant_not_found", Family::Auth, ErrorClass::AgentFault, false, "Check the namespace spelling and use a tenant that exists."),
    row("thread_not_found", Family::Share, ErrorClass::AgentFault, false, "List threads and use an id that exists."),
    row("timeout", Family::Internal, ErrorClass::HostFault, true, "The host timed out; retry shortly."),
    row("too_large", Family::Plan, ErrorClass::PlanLimit, false, "Shrink the payload to the limit in data.max_bytes."),
    row("too_many_invocations", Family::Args, ErrorClass::AgentFault, false, "Send at most data.limit invocations per call."),
    row("too_many_proposals", Family::Plan, ErrorClass::PlanLimit, false, "Wait for an open proposal to be decided; data.limit is the maximum."),
    row("too_many_rows", Family::Plan, ErrorClass::PlanLimit, false, "Narrow since/until so the export holds fewer rows than data.max_rows."),
    row("too_many_tools", Family::Plan, ErrorClass::PlanLimit, false, "Remove a tool or move to a plan with a higher tool limit."),
    row("tool_building", Family::Call, ErrorClass::HostFault, true, "Retry shortly; the tool's environment is still building."),
    row("tool_exception", Family::Call, ErrorClass::AgentFault, false, "Read data.exception_class and data.line, fix the tool's code, republish and re-run host.tool_test."),
    row("tool_missing", Family::Storage, ErrorClass::AgentFault, false, "Restore the target tool or remove the trigger."),
    row("tool_not_found", Family::Call, ErrorClass::AgentFault, false, "Check the name against host.tool_list; shared tools are called as <owner_namespace>.<tool>."),
    row("tool_oom", Family::Call, ErrorClass::AgentFault, false, "Reduce the tool's memory use or the input size."),
    row("tool_output_invalid", Family::Call, ErrorClass::AgentFault, false, "Return JSON that matches the tool's declared output."),
    row("tool_output_too_large", Family::Call, ErrorClass::AgentFault, false, "Return less data; the output exceeds the cap."),
    row("tool_process_limit", Family::Call, ErrorClass::AgentFault, false, "Reduce the number of processes or threads the tool starts."),
    row("tool_run_unsupported", Family::Call, ErrorClass::HostRefusal, false, "This kind cannot be run this way; call the tool through host.tool_call."),
    row("tool_timeout", Family::Call, ErrorClass::AgentFault, false, "Make the tool finish within the time cap, or reduce its input."),
    row("tool_trapped", Family::Call, ErrorClass::AgentFault, false, "The tool trapped; fix the runtime fault in its code and republish."),
    row("trace_not_found", Family::Storage, ErrorClass::AgentFault, false, "Use a call_id from a recent call; traces expire."),
    row("trigger_interval_too_short", Family::Plan, ErrorClass::PlanLimit, false, "Use a schedule at or above the plan's schedule_min_interval_s."),
    row("trigger_invalid", Family::Args, ErrorClass::AgentFault, false, "Fix the trigger field named in data.field."),
    row("trigger_kind_mismatch", Family::Args, ErrorClass::AgentFault, false, "Remove the existing trigger of the other kind before reusing the name."),
    row("trigger_not_found", Family::Storage, ErrorClass::AgentFault, false, "List triggers and use an id that exists."),
    row("trigger_quota_exceeded", Family::Plan, ErrorClass::PlanLimit, false, "Remove a trigger or move to a plan with a higher event_triggers_max."),
    row("typescript_not_supported", Family::Spec, ErrorClass::AgentFault, false, "Strip the types and publish plain JavaScript (ES2020); data.line is the first line with TypeScript syntax."),
    row("unknown_argument", Family::Args, ErrorClass::AgentFault, false, "Remove the argument; data.known lists the recognised ones."),
    row("unknown_import", Family::Spec, ErrorClass::AgentFault, false, "Use a module from the hint in data.hint; the import is not available in the sandbox."),
    row("unknown_kind", Family::Spec, ErrorClass::AgentFault, false, "Choose a kind from data.registered; data.did_you_mean suggests a near match."),
    row("unknown_spec_field", Family::Spec, ErrorClass::AgentFault, false, "Remove the spec field or use one the kind documents."),
    row("untested_tool_cannot_be_public", Family::Publish, ErrorClass::AgentFault, false, "Read data.verdict and data.detail, fix the tool, pass host.tool_test (data.next) for its current version, then set visibility public again."),
    row("unverifiable", Family::Call, ErrorClass::AgentFault, false, "Call data.call_instead for real; no dry run exists for this verb."),
    row("upstream_auth", Family::Upstream, ErrorClass::UpstreamFault, false, "Reconnect the upstream or fix the secret the tool sends; the upstream rejected the credential."),
    row("upstream_client_error", Family::Upstream, ErrorClass::UpstreamFault, false, "Fix the request the tool sends; the upstream rejected it as a client error."),
    row("upstream_error", Family::Upstream, ErrorClass::UpstreamFault, true, "Retry later; the upstream failed with a server error."),
    row("upstream_not_connected", Family::Upstream, ErrorClass::AgentFault, false, "Connect the upstream through the vault link first."),
    row("upstream_rate_limited", Family::Upstream, ErrorClass::UpstreamFault, true, "Wait and retry; the upstream is rate limiting this tool."),
    row("upstream_status", Family::Upstream, ErrorClass::UpstreamFault, false, "Read data.upstream_status for the upstream's HTTP status, then fix the request or retry later."),
    row("upstream_timeout", Family::Upstream, ErrorClass::UpstreamFault, true, "Retry later; the upstream did not answer in time."),
    row("upstream_unreachable", Family::Upstream, ErrorClass::UpstreamFault, true, "Retry later; the upstream could not be reached."),
    row("uptime_unavailable", Family::Internal, ErrorClass::HostRefusal, false, "This host has no http kind registered to probe with; do not retry."),
    row("validation", Family::Args, ErrorClass::AgentFault, false, "Fix the counter or field named in data.key."),
    row("vault_provider_not_found", Family::Oauth, ErrorClass::AgentFault, false, "Register the vault provider first or use a registered name."),
    row("webhook_body_too_large", Family::Plan, ErrorClass::PlanLimit, false, "Send a body under data.limit_bytes."),
    row("webhook_not_found", Family::Storage, ErrorClass::AgentFault, false, "Use a webhook that exists."),
    row("webhook_over_quota", Family::Plan, ErrorClass::PlanLimit, true, "Retry after the daily reset; the tenant is over its calls_per_day quota."),
    row("webhook_row_not_found", Family::Storage, ErrorClass::AgentFault, false, "Use an inbox row id that exists for this webhook."),
    row("webhook_signature_invalid", Family::Auth, ErrorClass::AgentFault, false, "Re-send with a signature that matches the webhook secret."),
    row("wrong_tenant", Family::Auth, ErrorClass::AgentFault, false, "Use the credential of the tenant this URL belongs to."),
];
