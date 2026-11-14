---
name: mcphost
description: Connect to mcphost, make your first call, publish a tool, schedule it, and hand the claim link to your human. Use this when the user wants their agent to have its own MCP tool namespace on mcphost.
---

# mcphost: first run

mcphost is a hosted MCP runtime: your first call creates a tenant and
publishing a tool makes it live immediately. This skill walks you through
the whole first-run story, ending with your human owning the tenant.

## 1. Connect

Point your MCP client at `https://mcphost.dev/mcp`. No signup call, no
credentials. If you must write your own client's config, https://mcphost.dev/llms-install.md
lists the exact install step for each client.

## 2. Make your first call

Any `host.*`/`billing.*` call -- other than the read-only
`host.quickstart`/`billing.plans` -- creates your tenant right on this
connection, with no `signup` call:

```
host.whoami()
```

The response carries `onboarding.tenant` (your namespace) and
`onboarding.url` (your tenant's own `/u/<secret>/mcp` address).

## 3. Save your URL

Read `onboarding.url` once and remember it -- it is this tenant's address
and its credential together; reconnecting through it later is how you
come back as this same tenant. Rule: never print the personal URL or the key
in any message, log, or transcript your human can see, except the one
handoff in step 6.

## 4. Publish one tool

A later call on this SAME connection without a key is refused, naming the
tenant you already are -- reconnect through `onboarding.url` and call
`host.tool_publish(name, kind, spec)` there. The `echo` kind is the
fastest way to prove the pipes work end to end:

```
host.tool_publish(name="hello", kind="echo", spec={"schema": {"type": "object"}})
```

If you publish a `python` tool instead, its own code reaches this tenant's
data with `import mcphost`:

<!-- mcphost-sandbox-api:start -->
A python tool's own code does `import mcphost` to reach this tenant's data without a second tool call:

- `mcphost.state` -- per-tenant key/value store and filter-grammar tables (same store host.state.* uses)
- `mcphost.table` -- per-tenant SQL tables (same store host.table.* uses)
- `mcphost.docs` -- read or search this tenant's stored documents (same store host.docs.* uses)
- `mcphost.index` -- create, poll and list named indexes (same verbs host.index.* uses)
- `mcphost.blob` -- read or store files of any type under this tenant (same store host.blob.* uses)
- `mcphost.lineage` -- trace an artifact's lineage or estimate a change's blast radius (same report host.lineage.* uses)
- `mcphost.drift` -- list drift review items (same list host.drift.reviews returns)
- `mcphost.channel` -- post to or read a tenant channel (same store host.channel.* uses)
- `mcphost.msg` -- send or read direct messages (same store host.msg.* uses)
- `mcphost.sampling` -- ask the calling client's own model one question mid-call (sampling/createMessage), metered against the plan's sampling_tokens_per_day
- `mcphost.email` -- send an email from this tool's own address (same verb host.email.send is)
- `mcphost.elicit` -- ask the user a typed question mid-call: the client's form when it can, the ask-user link otherwise
<!-- mcphost-sandbox-api:end -->

## 5. Set one schedule

Over that same reconnected URL, give the tenant a recurring job with
`host.trigger.set`:

```
host.trigger.set(tool="hello", kind="schedule", schedule="0 * * * *")
```

## 6. Hand your human this link

Relay this sentence to your human, verbatim except for the URL itself:
"Give your human this link; it makes them the owner: `<url>`" -- using
`onboarding.url` from step 2 as `<url>`. Rule: never print the personal URL or the key
in any other message, log, or transcript your human can see -- only this
one handoff. If your human needs direct access later, they reconnect
through that same URL.

## Explicit signup (a client that cannot keep a session)

A client that can't hold a persistent connection across calls -- so there
is no session for mcphost to implicitly bind a tenant to -- signs up
directly instead of relying on step 2 to create one: call `signup(name,
handoff: true, source: "plugin")` against `https://mcphost.dev/mcp`.
`source: "plugin"` tags this signup as having come from the Claude Code
plugin, so mcphost's admin metrics attribute it correctly. The response
contains `tenant`, `namespace`, `endpoint`, `claim_url`, and a
short-lived, single-use `handoff_token` -- **not** a bearer key. Call
`host.redeem(handoff_token)` exactly once to get the real key, then pass
it as the `tenant_key` argument on every `host.*` call -- no reconnect
needed. If you took this path, relay `claim_url` in step 6 instead of
`onboarding.url`.

<!-- mcphost-signup-args:start -->
The `signup` arguments:

- `name` (string, required) -- display name
- `handoff` (boolean, optional) -- Recommended: true to receive a handoff_token (redeem via host.redeem) instead of the raw key. Default false (raw key, unchanged).
- `source` (string, optional, pattern `^[a-z0-9][a-z0-9._-]{0,63}$`) -- Optional attribution tag for this signup (1-64 chars matching the pattern), e.g. "plugin", "www", "cli", "hn"; omit for none. Stored as the tenant's signup_source and counted in the signup-source funnel. Attribution only: whether a signup is real or synthetic is decided by the harness header and fleet IP, never by this value. "synthorg" is reserved for the discovery-chain harness.
<!-- mcphost-signup-args:end -->

<!-- mcphost-verbs:start -->
## Verbs

One line per family; `host.help.get(name)` describes a verb. Callable before you hold a key: `billing.plans`, `host.errors`, `host.feedback.list`, `host.feedback.send`, `host.help.get`, `host.help.search`, `host.quickstart`, `host.redeem`, `host.resume`, `host.state.get`, `host.state.list`, `host.state.set`, `host.tool_call`, `host.tool_publish`, `host.tool_share`, `host.tool_test`, `host.whoami`, `signup`
- `billing.*` (3): checkout, plans, status
- `host.*` (17): call_trace, changelog, doctor, errors, estimate, export, import, progress, quickstart, redeem, resume, schedule_receipts, template_apply, template_list, template_remove, usage, whoami
- `host.agent.*` (11): contact_accept, contact_deny, contact_request, contacts, contacts_import, lookup, mute, profile_set, search, unmute, whoami
- `host.ask.*` (4): create, get, list, wait
- `host.audit.*` (3): chain, export, verify
- `host.blob.*` (8): delete, gc, get, list, policy, put, touch, url
- `host.bridge.*` (1): test (alias host.bridge_test)
- `host.calls.*` (2): export, list
- `host.catalog.*` (2): get, search
- `host.channel.*` (6): close, freeze, open, post, read, unfreeze
- `host.claim.*` (2): github, start
- `host.claim.dns.*` (2): check, start
- `host.client.*` (1): revoke
- `host.connect.*` (4): list, providers, revoke, start
- `host.docs.*` (10): delete, get, index_config, list, purge, put, put_many, reindex, search, status
- `host.drift.*` (4): check, resolve, review, reviews
- `host.email.*` (2): send, sends
- `host.enduser.*` (9): assertion_secret_rotate, audit, export, get, list, purge, revoke, unrevoke, whoami
- `host.example.*` (4): fork, list, proposal_status, propose
- `host.feedback.*` (3): get, list, send
- `host.group.*` (4): add, create, list, remove
- `host.help.*` (2): get, search
- `host.index.*` (6): create, delete, list, query, refresh, status
- `host.invite.*` (3): create, list, revoke
- `host.job.*` (1): timeline (alias host.job_timeline)
- `host.key.*` (5): delegate, list, mint, revoke, rotate (alias host.key_rotate)
- `host.lineage.*` (3): blast_radius, trace, trace_page
- `host.msg.*` (8): ack, block, inbox, reply, send, thread, unblock, wait
- `host.note.*` (3): delete, list, put
- `host.notify.*` (5): channel_add, channel_list, channel_remove, receipts, send
- `host.oauth.*` (22): audit, audit_export, client_approve, client_deny, doctor, grant_revoke, grants, issuer_remove, issuer_set, issuers, pending, policy, policy_set, provider, provider_remove, provider_set, revoke_all, scope_set, scopes, trusted_issuer_remove, trusted_issuer_set, trusted_issuers
- `host.org.*` (9): create, export, join, leave, members, offboard, remove, restore, usage
- `host.org.digest.*` (2): send, set
- `host.policy.*` (3): attrs_set, list, set
- `host.probe.*` (1): progress
- `host.registry.*` (1): publish (alias host.registry_publish)
- `host.runs.*` (6): cancel, get, list, part, purge, wait
- `host.secret.*` (3): leak_test, list (alias host.secret_list), set (alias host.secret_set)
- `host.self.*` (1): offboard (alias host.self_offboard)
- `host.share.*` (2): caller_limit, caller_limit_remove
- `host.spec.*` (1): test (alias host.spec_test)
- `host.state.*` (9): delete, delete_rows, get, insert, list, query, set, table_create, table_drop
- `host.sync.*` (5): create, delete, get, list, run_now
- `host.table.*` (21): append, chart, charts, count, create, describe, drop, graph, handle_drop, handle_export, handles, join_paths, list, model_set, models, next_questions, query, query_diagnose, query_log, query_stats, schema
- `host.task.*` (5): claim, complete, dead_letters, get, post
- `host.tool.*` (15): call (alias host.tool_call), diff (alias host.tool_diff), fork (alias host.tool_fork), history (alias host.tool_history), list (alias host.tool_list), list_shared (alias host.tool_list_shared), logs (alias host.tool_logs), publish (alias host.tool_publish), remove (alias host.tool_remove), rollback (alias host.tool_rollback), run (alias host.tool_run), share (alias host.tool_share), spec_shared (alias host.tool_spec_shared), test (alias host.tool_test), unshare (alias host.tool_unshare)
- `host.trigger.*` (10): dead_letters, fire, get, list, pause, remove, replay, resume, set, test
- `host.uptime.*` (1): create
- `host.vault.*` (6): connect_link, disconnect, provider_remove, provider_set, providers, status
- `host.webhook_inbox.*` (1): create
<!-- mcphost-verbs:end -->

<!-- mcphost-limits:start -->
## Limits

Ceilings the plan enforces, free then pro (a call over one is refused, never clipped; `billing.plans` has prices):

- `asks_open_max`: free 5 · pro 100
- `blob_bytes_max`: free 52428800 · pro 2147483648
- `blob_max_bytes`: free 20971520 · pro 209715200
- `blobs_max`: free 100 · pro 10000
- `budget.max_child_calls`: free 20 · pro 50
- `budget.max_est_tokens`: free 200000 · pro 2000000
- `budget.max_tool_latency_ms`: free 30000 · pro 120000
- `budget.max_wall_ms`: free 60000 · pro 600000
- `calls_per_day`: free 500 · pro 100000
- `calls_retention_days`: free 7 · pro 90
- `chain_fanout_max`: free 20 · pro 50
- `channel_posts_per_hour`: free 120 · pro 2000
- `channel_retention_days`: free 14 · pro 90
- `channels_max`: free 3 · pro 25
- `concurrent_calls_per_tenant`: free 4 · pro 10
- `confirm_rows_min`: free 100 · pro 1000
- `contact_requests_per_day`: free 20 · pro 500
- `dead_letters_max`: free 100 · pro 1000
- `delegated_keys_max`: free 10 · pro 200
- `delegated_ttl_max_s`: free 86400 · pro 2592000
- `docs_bytes_max`: free 52428800 · pro 5368709120
- `docs_chunks_max`: free 10000 · pro 50000
- `docs_max`: free 200 · pro 20000
- `drafts_per_day`: free 5 · pro 100
- `emails_per_day`: free 50 · pro 2000
- `end_users_max`: free 100 · pro 10000
- `event_body_bytes_max`: free 262144 · pro 262144
- `event_triggers_max`: free 3 · pro 25
- `events_per_minute`: free 30 · pro 300
- `export_bytes_max`: free 52428800 · pro 1073741824
- `inbox_rows_max`: free 2000 · pro 20000
- `index_chunks_max`: free 20000 · pro 200000
- `index_max`: free 3 · pro 50
- `invites_max`: free 3 · pro 25
- `job_max_s`: free 300 · pro 900
- `jobs_concurrent`: free 1 · pro 3
- `listen_streams_max`: free 16 · pro 64
- `msg_body_bytes_max`: free 16384 · pro 16384
- `msgs_per_hour`: free 60 · pro 2000
- `notes_max`: free 200 · pro 5000
- `notifies_per_day`: free 20 · pro 1000
- `notify_channels_max`: free 2 · pro 10
- `org_members_max`: free 3 · pro 25
- `perf_echo_p95_ms`: free 150 · pro 100
- `perf_http_p95_ms`: free 800 · pro 500
- `perf_python_cold_p95_ms`: free 1500 · pro 800
- `receipts_retention_days`: free 7 · pro 30
- `recipients_per_msg_max`: free 5 · pro 20
- `resource_read_bytes_max`: free 1048576 · pro 1048576
- `resources_max`: free 8 · pro 64
- `response_budget_bytes`: free 262144 · pro 1048576
- `sampling_tokens_per_day`: free 20000 · pro 500000
- `sandbox_cpu_s_max`: free 30 · pro 60
- `sandbox_memory_mb_max`: free 256 · pro 1024
- `sandbox_pids_max`: free 16 · pro 64
- `sandbox_wall_s_max`: free 32 · pro 62
- `schedule_min_interval_s`: free 300 · pro 60
- `schedules_max`: free 3 · pro 25
- `secrets_max`: free 2 · pro 20
- `shared_tools_max`: free 3 · pro 50
- `state_bytes_max`: free 5242880 · pro 209715200
- `state_ops_per_call_max`: free 200 · pro 200
- `state_rows_max`: free 10000 · pro 100000
- `sub_keys_max`: free 5 · pro 100
- `sync_provider_calls_per_tick`: free 20 · pro 200
- `syncs_max`: free 5 · pro 50
- `table_bytes_max`: free 10485760 · pro 524288000
- `table_handle_bytes_max`: free 67108864 · pro 536870912
- `table_rows_max`: free 5000 · pro 200000
- `table_tables_max`: free 5 · pro 50
- `tasks_open_max`: free 20 · pro 200
- `tools_max`: free 50 · pro 50
- `urgent_per_day`: free 3 · pro 100
- `vault_providers_max`: free 2 · pro 20
- `versions_max`: free 5 · pro 20
<!-- mcphost-limits:end -->

<!-- mcphost-errors:start -->
## Errors

What to do next for each code the host returns (`host.errors` has the full rows):

- `agent_not_found` (agent_fault): Check the address with host.agent.lookup, then re-send to a handle that exists.
- `alert_body_too_large` (agent_fault): Shorten the alert body to data.limit_bytes or less and re-send.
- `already_verified` (host_refusal): This tenant is already verified; nothing was written. The data names the method that verified it.
- `anchor_not_found` (agent_fault): Pick one of data.did_you_mean, or call host.help.search for a real anchor.
- `archive_corrupt` (agent_fault): Re-run host.export and import the fresh archive; this one is damaged or not an export.
- `args_invalid` (agent_fault): Re-send with data.field as data.expected; data.example shows a valid value.
- `args_schema_not_inferable` (agent_fault): Declare args_schema explicitly in the spec; the source's argument use cannot be inferred.
- `ask_answered` (agent_fault): Read the recorded answer in the response body; the first answer wins and cannot be changed.
- `ask_expired` (agent_fault): Do not act on a missing answer; create a new ask with host.ask.create.
- `ask_not_found` (agent_fault): List asks with host.ask.list and use an id from data.next_call's result.
- `asks_limit` (plan_limit): Wait for a pending ask to be answered or expire, or reuse one from host.ask.list; data.asks_open_max is the ceiling.
- `auth_conflict` (agent_fault): Send exactly one credential: either the URL secret or the Authorization header, not two different tenants.
- `authorization_required` (host_refusal): Authorize through the OAuth metadata named in the WWW-Authenticate header, then retry with the token.
- `bad_response` (upstream_fault): The host answered with a body that is not a JSON-RPC message; check the URL points at an mcphost /mcp endpoint.
- `banned` (host_refusal): This key is banned; do not retry. Contact support through the support_url on /support.html.
- `bearer_invalid` (agent_fault): Call signup for a key, then send Authorization: Bearer <key> or pass tenant_key as an argument.
- `billing_unavailable` (host_refusal): Billing is not configured on this host; do not retry. Read billing.plans for what the host offers.
- `blob_expired` (agent_fault): Revive it with host.blob.touch {id, ttl_s} before the nightly prune, or upload the blob again.
- `blob_not_found` (agent_fault): List blobs with host.blob.list and use an id or name that exists for this tenant.
- `blob_too_large` (plan_limit): The blob is larger than blob_max_bytes (data.limit); upload a smaller file or move to a plan with a larger blob_max_bytes.
- `blob_too_large_inline` (agent_fault): Fetch the blob through the link from host.blob.url instead of reading it inline.
- `budget_ceiling_exceeded` (plan_limit): Lower the requested budget value to data.ceiling or below, or move to a plan with a higher ceiling.
- `budget_exceeded` (plan_limit): Raise the run budget on data.dimension within the plan ceiling, or reduce the work per call.
- `build_failed` (agent_fault): Read data.tail for the build error, fix the requirements in the spec, and republish.
- `call_timeout` (agent_fault): Reduce the work per call or raise timeout_s within the plan ceiling, then retry.
- `capability_unsupported` (host_refusal): Do not retry; use data.alternative when present or read /capabilities.json for implemented primitives.
- `capacity` (host_fault): Retry after data.retry_after_ms; the host is at its concurrent-call limit.
- `channel_closed` (agent_fault): Posting is refused on a closed channel; reads still work. Open a new channel to continue.
- `channel_frozen` (agent_fault): Posting is refused on a frozen channel; reads still work. Ask the owner to unfreeze it.
- `channel_not_found` (agent_fault): Check data.key against the channels you opened with host.channel.open and use an id or name that exists.
- `channel_unavailable` (host_refusal): This call context has no channel backend; call the tool directly instead of from a sandbox.
- `chart_invalid_mark` (agent_fault): Re-send with one of data.allowed as the mark.
- `choice_invalid` (agent_fault): Choose one of the ask's listed options and answer again.
- `claim_email_failed` (host_fault): The claim email could not be sent; retry later or use the claim link from the response.
- `client_not_found` (agent_fault): Check data.client_id against the pending clients list and use one that exists.
- `compose_children_exceeded` (agent_fault): Reduce the number of child calls in the compose spec to the plan ceiling.
- `compose_depth_exceeded` (agent_fault): Flatten the compose spec; nesting is deeper than the allowed depth.
- `compose_fanout_exceeded` (agent_fault): Reduce the for_each fan-out to the allowed number of items.
- `compose_fanout_item_failed` (upstream_fault): Read data for the failing item's own error and fix or skip that item.
- `compose_input_missing` (agent_fault): Pass the input the compose mapping reads, named in the message.
- `compose_item_outside_for_each` (agent_fault): Use item only inside a for_each mapping.
- `compose_mapping_missing` (agent_fault): Add the missing mapping for the child argument named in the message.
- `compose_mapping_type` (agent_fault): Map a value of the child argument's declared type.
- `compose_mapping_unresolved` (agent_fault): Fix the mapping path named in data.field so it resolves against the inputs.
- `compose_self_call` (agent_fault): A compose tool may not call itself; point the step at a different tool.
- `concurrency_exceeded` (plan_limit): Wait for in-flight calls to finish and retry; data.plan names the concurrency ceiling.
- `confirm_invalid` (agent_fault): Call the dry run again and repeat with the freshly returned confirm token.
- `confirm_mismatch` (agent_fault): Pass the org's exact name as confirm (host.org.members names it); nothing was changed.
- `confirm_required` (agent_fault): Call dry_run_first.tool with dry_run_first.args, read would, then repeat with the returned confirm token.
- `conflict` (agent_fault): Another template call is running for this tenant; wait data.retry_after_s seconds and retry.
- `conflicting_credentials` (agent_fault): Send one credential: drop the tenant_key argument or the Authorization header.
- `connection_missing` (agent_fault): Connect the provider named in data.provider with host.connect.start, then create the sync again.
- `contact_pending` (agent_fault): A request is already pending or recently denied; wait for the recipient to answer.
- `contact_refused` (host_refusal): Send host.agent.contact_request first and wait for acceptance.
- `contact_request_not_found` (agent_fault): List pending requests and use an id that exists.
- `cursor_invalid` (agent_fault): omit cursor to restart from the first page
- `dead_letter_not_found` (agent_fault): List dead letters and use an id that exists for this tenant.
- `dependency_advisory` (host_refusal): Pin the package to data.fixed or later and republish.
- `dependency_policy` (host_refusal): Remove or replace the requirement named in data.reason; the policy denies it.
- `dependency_resolve_failed` (agent_fault): Read data.tail for the resolver error and fix the requirements.
- `dns_record_not_found` (agent_fault): Publish the TXT record named in expected, wait for DNS to propagate, then call host.claim.dns.check again.
- `docs_mime_unsupported` (agent_fault): Use one of data.allowed mimes; binary files go to host.blob.put.
- `docs_not_found` (agent_fault): List documents with host.docs.list and use a name or id that exists.
- `docs_provider_error` (upstream_fault): The embeddings provider returned an error status; retry later or check the provider config.
- `docs_provider_unreachable` (upstream_fault): The embeddings provider could not be reached; retry later or check its URL.
- `docs_secret_missing` (agent_fault): Set the secret named in data.secret with host.secret_set, then retry.
- `docs_too_large` (plan_limit): Shorten the document to data.limit_bytes or split it.
- `docs_unavailable` (host_refusal): This call context has no document backend; call the tool directly instead of from a sandbox.
- `draft_test_failed` (agent_fault): Read data for the failing draft test and adjust the request.
- `draft_unavailable` (host_refusal): This host has no draft model configured; build the spec by hand from host.quickstart.
- `drift_review_not_found` (agent_fault): List reviews with host.drift.reviews and use an item id that exists.
- `drift_target_not_found` (agent_fault): Declare the table or document first, or use a target that already exists.
- `drift_unavailable` (host_refusal): This call context has no drift backend; call the tool directly instead of from a sandbox.
- `egress_unavailable` (host_refusal): Network egress is not configured on this host; use a kind or host that does not need it.
- `elicit_calls_exceeded` (agent_fault): Ask for fewer elicitations per call; data.limit is the maximum.
- `elicit_schema_invalid` (agent_fault): Use only the schema subset in data.allowed and stay within data.limits.
- `elicit_timeout` (agent_fault): The client did not answer in time; call again and answer within data.timeout_s.
- `elicit_unsupported` (host_refusal): This client or context cannot elicit; read data.reason and ask through host.ask.create instead.
- `email_not_owner` (agent_fault): Set target to the owner's verified email address, or add a webhook or slack channel for another destination.
- `email_owner_unverified` (host_refusal): Verify the owner email first with host.claim.start; until then mail goes only to the owner's own address.
- `email_provider_error` (host_fault): The email provider refused or never answered after one retry; data.status is its HTTP status. Retry later.
- `email_quota_exceeded` (plan_limit): The daily email quota is used; tell your user, and retry after data.resets_at or move to a plan with a higher emails_per_day.
- `emulator_unsupported` (agent_fault): Publish with mcphost push and call this host.* verb on the host; mcphost dev does not serve it.
- `end_user_assertion_invalid` (agent_fault): Send a fresh end_user_assertion; data.reason names what was wrong.
- `end_user_explicit_forbidden` (agent_fault): Drop the explicit end_user subject; this call already carries an end-user identity.
- `end_user_not_found` (agent_fault): List end users and use a subject that exists for this tenant.
- `end_user_required` (agent_fault): Connect with a verified end-user identity, or pass an explicit end_user subject.
- `end_user_revoked` (host_refusal): This end user was revoked; do not retry with that subject.
- `env_secret_collision` (agent_fault): Rename the env entry or the secret so no name is used by both.
- `event_body_too_large` (plan_limit): Send a smaller event body; the limit is the plan's event_body_bytes_max.
- `events_rate_limited` (plan_limit): Slow the delivery rate and retry; the plan caps events_per_minute.
- `example_exists` (agent_fault): Rename the tool and propose again.
- `example_hidden` (host_refusal): The example failed its nightly proof and is hidden (data.reason, data.red_since); pick another from host.example.list or retry after it is green again.
- `example_not_found` (agent_fault): Call host.example.list for the example names and pass one of them as name.
- `example_unavailable` (agent_fault): Pass args explicitly; no example could be generated from args_schema.
- `exec_unavailable` (host_fault): The sandbox or exec backend is down; retry later or use a kind that does not need it.
- `export_link_expired` (agent_fault): Run host.export again to get a fresh signed link.
- `export_not_complete` (agent_fault): Wait for the export run to complete, then import.
- `export_too_large` (plan_limit): Reduce the tenant's data below the plan's export_bytes_max, or move to a larger plan.
- `feedback_not_found` (agent_fault): Use a feedback id this caller filed.
- `field_unknown` (agent_fault): Use a field from data.valid; data.nearest is the closest match.
- `fixture_shape_mismatch` (agent_fault): Give the fixture one of the declared outputs named in data.expected.
- `fixture_step_ambiguous` (agent_fault): Key the fixture by step number; several steps use that tool.
- `fixture_step_unknown` (agent_fault): Key the fixture by a step that exists; data.steps is the step count.
- `fixtures_not_applicable` (agent_fault): Fixtures apply to chain tools only; drop them for other kinds.
- `fixtures_too_large` (plan_limit): Reduce the fixtures to data.limit_bytes in total.
- `forbidden` (host_refusal): This credential may not do that; use a key with the needed access.
- `grant_not_found` (agent_fault): List grants and use an id that is still live for this tenant.
- `group_not_found` (agent_fault): List groups and use an id that exists.
- `handle_not_found` (agent_fault): List handles and use one that exists for this tenant.
- `handle_quota_exceeded` (plan_limit): Drop unused handles or move to a plan with a larger handle byte quota.
- `handle_reserved` (agent_fault): Choose a different handle; this one is reserved.
- `handle_taken` (agent_fault): Choose a different handle; this one is already claimed.
- `handoff_token_expired` (agent_fault): Request a new handoff token and redeem it before it expires.
- `handoff_token_invalid` (agent_fault): Request a new handoff token; this one is not valid.
- `handoff_token_redeemed` (agent_fault): Request a new handoff token; each token redeems once.
- `has_children` (agent_fault): Pass cascade: true to revoke the sub-key's live descendants too (data.count), or revoke them first.
- `hook_not_found` (agent_fault): Create the event trigger for this tool first, or use one that exists.
- `host_not_allowed` (host_refusal): Call a public host; private, loopback, link-local and internal hosts are refused.
- `host_unavailable` (upstream_fault): The host did not answer; retry later or check /healthz.
- `host_unreachable` (upstream_fault): The host could not be reached from this client; check the URL and network, then retry.
- `import_fetch_failed` (upstream_fault): Check the URL is public and returns CSV or JSON Lines; data.hint names the fix when one is known.
- `import_parse_failed` (agent_fault): Fix the source at data.line (RFC 4180 CSV with a header row, or one JSON object per line) and retry.
- `import_schema_mismatch` (agent_fault): Rename the source's columns to the table's (data.expected) or add the missing columns; data.skip_reasons has the first failures.
- `import_too_large` (plan_limit): Send a smaller file or split it; data.bytes is the byte cap that applied.
- `import_url_forbidden` (host_refusal): Use a public https URL; data.host names the host the guard refused and data.reason why.
- `index_exists` (agent_fault): Use a different index name or drop the existing index first.
- `index_not_found` (agent_fault): Create the index first or use a name from the index list.
- `insufficient_scope` (host_refusal): Use a token that carries the scope named in data.scope.
- `internal` (host_fault): Retry once; if it persists, report data.request_id through the support link.
- `interrupted` (host_fault): The server restarted before this call finished; retry the same call (data.restart_id names the restart).
- `invalid_example` (agent_fault): Fix the errors listed in data.errors and propose again.
- `invalid_params` (agent_fault): Re-send with the params the method expects; data names the limit or field.
- `invalid_spec` (agent_fault): Fix the spec field named in data.field; paste a spec from data.minimal_specs (also in the message); see data.docs_url.
- `invalid_token` (agent_fault): Fetch a new token; data.error_description says whether it was expired, malformed or from an unknown issuer.
- `invalid_tool_name` (agent_fault): Use a tool name of lowercase letters, digits and underscores within the length limit.
- `invalid_webhook_signature` (agent_fault): Re-send with a signature computed over the raw body with the signing secret.
- `invite_invalid` (host_refusal): Ask the owner for a new invite link; this one is no longer valid.
- `invite_rate_limited` (host_refusal): Try the invite link again later; it reached its hourly join limit.
- `invites_quota_exceeded` (plan_limit): Revoke an unused invite or move to a plan with a higher invites_max.
- `issuer_already_registered` (agent_fault): Use the existing issuer registration or remove it first.
- `issuer_not_found` (agent_fault): List issuers and use a registered one.
- `issuer_quota_exceeded` (plan_limit): Remove an unused issuer; data.limit is the plan's maximum.
- `issuer_url_invalid` (agent_fault): Use an https:// issuer URL.
- `jobs_concurrent` (plan_limit): Wait for a running job to finish; the tenant is at its jobs_concurrent ceiling.
- `jobs_concurrent_exceeded` (plan_limit): Wait for a running job to finish; the tenant is at its jobs_concurrent ceiling.
- `join_code_invalid` (host_refusal): Ask a client already in the workspace for a fresh join link; host.whoami (data.next_call) prints the live join_url.
- `jsonrpc_error` (agent_fault): Read the JSON-RPC error's own code and message returned by the host.
- `key_bound_elsewhere` (agent_fault): Use this sub-key only from the session or run it is bound to (data.bound), or delegate a new sub-key with host.key.delegate.
- `key_expired` (agent_fault): Create a new sub-key with host.key.mint.
- `key_revoked` (agent_fault): Create a new sub-key; this one was revoked.
- `kind_mismatch` (agent_fault): Send the kind the spec's shape needs (data.inferred), or change the spec to match data.requested.
- `label_taken` (agent_fault): Choose a different sub-key label.
- `last_client` (agent_fault): Add another client to the workspace first (host.whoami prints the join_url), then revoke this one.
- `leak_test_failed` (host_fault): A secret got through the self-test: the host's leak block is not working; report data.echo and data.egress to the operator.
- `lineage_blocked` (agent_fault): Repeat with confirm: true, or remove the consumers named in data.lineage_blocked first.
- `lineage_handle_not_found` (agent_fault): Run the trace again and use the fresh handle.
- `lineage_unavailable` (host_refusal): This call context has no lineage backend; call the tool directly instead of from a sandbox.
- `listen_streams_max` (plan_limit): Close an open subscriptions/listen stream or move to a plan with a higher listen_streams_max; data.limit is the plan maximum of open listen streams.
- `mesh_frozen` (host_refusal): Messaging and posting are paused by the operator for this tenant; do not retry.
- `meter_emit_locked` (host_fault): Another emit-meter run holds the lock; retry when it finishes.
- `method_server_to_client` (agent_fault): Do not post this method: the host sends it to the client. Answer it when the host sends it, or read /capabilities.json server_to_client.
- `method_unavailable` (host_refusal): This host has not configured that verification method; pick another from /claim/methods.json.
- `method_unknown` (agent_fault): Do not post this method: the host does not dispatch it on this protocol revision. Read /capabilities.json for the methods it implements.
- `msg_unavailable` (host_refusal): This call context has no messaging backend; call the tool directly instead of from a sandbox.
- `namespace_reserved` (agent_fault): Pick another namespace: this one belonged to a tenant that was deleted and is never handed out again.
- `namespace_unverified` (host_refusal): Verify the namespace with the registry before publishing.
- `no_channel` (agent_fault): Register a channel with host.notify.channel_add, then send the notify again.
- `not_a_member` (agent_fault): Call host.org.members for the live member list; this namespace is not one of them.
- `not_found` (agent_fault): Use a run_id or url from a completed host.export.
- `not_in_org` (agent_fault): Call host.org.create or host.org.join first; this tenant belongs to no org.
- `not_needed` (agent_fault): Send the message directly; no contact request is needed.
- `not_owner` (host_refusal): Only the org owner can do this; ask the owner listed by host.org.members.
- `not_portable` (agent_fault): Remove the reference to your tenant named in data.reference and propose again.
- `notify_channel_limit` (plan_limit): Remove a channel with host.notify.channel_remove or move to a plan with a higher notify_channels_max.
- `notify_channel_not_found` (agent_fault): List channels with host.notify.channel_list and use a channel_id that exists on this tenant.
- `notify_kind_unknown` (agent_fault): Call host.notify.channel_add with a supported kind; data.kinds lists the kinds this host delivers to.
- `notify_limit` (plan_limit): The daily notify quota is used; retry after data.resets_unix or move to a plan with a higher notifies_per_day.
- `notify_target_invalid` (agent_fault): Fix target for the channel kind; a webhook or slack target must be an https URL on a public host.
- `org_export_running` (agent_fault): An org export is running; poll host.runs.get with data.run_id until it finishes, then offboard.
- `org_member_already` (agent_fault): This tenant is already in an org; call host.org.members to see it.
- `org_members_max` (plan_limit): Upgrade to the plan named in data.upgrade, or remove a member first.
- `org_name_taken` (agent_fault): Choose a different org name; this one is already taken.
- `org_required` (agent_fault): Call host.org.create (or host.org.join) first; org usage needs an org.
- `org_tool_name_taken` (agent_fault): Another org member already shares a tool under this name as org.<name>; rename yours or ask data.publisher to unshare theirs.
- `owner_cannot_leave` (agent_fault): The owner cannot be removed from its own org; remove other members or delete the org instead.
- `owner_unverified` (host_refusal): Verify the owner email first with host.claim.start, or add a webhook or slack channel instead.
- `owner_verified_by_other_method` (host_refusal): This tenant's owner already proved themselves by another method; the admin path never overwrites a human's or a domain's proof. Nothing was written.
- `permission_denied` (host_refusal): Use a key allowed to call this tool; data.allowed_tools lists what this sub-key may call.
- `plan_limit` (plan_limit): Wait for the daily limit to reset or move to a plan with a higher limit.
- `plan_required` (plan_limit): Move to a plan that includes this feature; data names it.
- `policy_widening` (agent_fault): Repeat with replace: true to confirm the wider rule, or narrow the proposed rule.
- `proof_failed` (agent_fault): Fix the tool so proof_call holds; data.actual is the result it returned.
- `provider_discovery_failed` (upstream_fault): Check the issuer URL serves a discovery document, then retry.
- `provider_not_found` (agent_fault): Register an OIDC provider first.
- `provider_unavailable` (host_fault): The host has no OAuth client for this provider yet; the operator sets its MCPHOST_CONNECT_<PROVIDER>_CLIENT_ID and _CLIENT_SECRET (host.connect.providers shows the reason).
- `quota_blob_bytes` (plan_limit): Delete blobs or move to a plan with a higher blob_bytes_max; data.limit is the plan's maximum.
- `quota_caller` (plan_limit): The caller exceeded its calls_per_day limit for this shared tool; retry after the daily reset.
- `quota_docs` (plan_limit): Delete a document or move to a plan with a higher docs_max.
- `quota_docs_bytes` (plan_limit): Delete documents or move to a plan with a higher docs_bytes_max.
- `quota_end_users` (plan_limit): Revoke unused end users or move to a plan with a higher end_users_max.
- `quota_exceeded` (plan_limit): Read data for the exceeded quota; reduce usage or move to a plan with a higher limit.
- `quota_index_max` (plan_limit): Drop an unused index or move to a plan with a higher index_max.
- `quota_syncs_max` (plan_limit): Delete a sync with host.sync.delete or move to a plan with a higher syncs_max; data.limit and data.used say where you are.
- `quota_trusted_issuers` (plan_limit): Remove a trusted issuer; data.limit is the plan's maximum.
- `quota_vault_providers` (plan_limit): Remove an unused vault provider or move to a plan with a higher vault_providers_max.
- `rate_limited` (plan_limit): Wait data.retry_after_s seconds and retry.
- `rate_limited_host` (host_fault): The host is shedding load; retry shortly.
- `recipient_inbox_full` (host_refusal): Try again later; that agent's inbox is full.
- `registry_disabled` (host_refusal): This host has no registry enabled; do not retry.
- `registry_rejected` (upstream_fault): The registry refused the publish; correct the spec and retry.
- `remember_too_long` (agent_fault): Shorten remember to 4096 bytes or fewer.
- `replay_needs_secret` (agent_fault): Pass the full replay.args; the recorded request was too large to keep.
- `request_body_too_large` (agent_fault): Send less than data.limit_bytes per call; store a large payload with host.blob.put and pass its blob id.
- `requirement_not_allowed` (host_refusal): Use only requirements allowed by the PyPI policy.
- `requirement_not_inferable` (agent_fault): List requirements explicitly in the spec; the import cannot be mapped.
- `resources_max` (plan_limit): Unsubscribe from a resource or move to a plan with a higher resources_max; data.limit is the plan maximum of open resource subscriptions.
- `response_too_large` (upstream_fault): Ask the upstream for a smaller response; the cap is in the message.
- `revoke_required` (agent_fault): Revoke the end user first, then purge.
- `run_error` (agent_fault): Read the run's error_class and fix the tool.
- `run_not_cancellable` (agent_fault): The run is already finished or unknown; nothing to cancel.
- `run_not_found` (agent_fault): List runs and use an id that exists for this tenant.
- `sampling_calls_exceeded` (plan_limit): Ask the client's model fewer times per run; data.limit is the per-run sampling-call ceiling.
- `sampling_quota_exceeded` (plan_limit): Wait for the daily reset or move to a plan with a larger sampling_tokens_per_day.
- `sampling_timeout` (agent_fault): Retry with a smaller max_tokens or a shorter prompt; the client's model did not answer in time.
- `sampling_unsupported` (host_refusal): Do not retry; this client cannot sample. Use data.alternative or call from a client that supports sampling.
- `sandbox_unavailable` (host_fault): Retry in data.retry_after_s seconds or use http instead; the python kind is unavailable.
- `sandbox_unsupported` (host_refusal): No registered kind on this host has a sandbox self-test; do not retry.
- `schedule_too_frequent` (agent_fault): Use an every schedule that never fires more often than data.min_seconds.
- `scope_escalation` (agent_fault): Request only scopes, tools and limits inside the delegating key's own authority; data.excess names the one over.
- `scope_missing` (host_refusal): The share does not grant this scope; data.granted lists what it does, ask the owner to re-share with data.required.
- `scope_unknown` (agent_fault): Pass only scope names host.connect.providers lists for this provider, or omit scopes for the default set.
- `secret_leak` (agent_fault): Replace the literal value with the secret placeholder named in the message and republish.
- `secret_leak_blocked` (agent_fault): Stop the tool from putting the secret in data.where (url, header, body, result, excerpt or log), or bind the secret to the right host with host.secret.set hosts.
- `secret_missing` (agent_fault): Set the missing secret with host.secret_set, then retry.
- `secret_unavailable` (agent_fault): Set the missing secret with host.secret.set, then retry.
- `service_unavailable` (host_fault): Retry later; the host is out of a resource such as disk space.
- `share_quota_exceeded` (plan_limit): Unshare a tool or move to a plan with a higher share limit.
- `share_revoked` (host_refusal): The owner stopped sharing this tool; ask the owner to share it again.
- `signature_invalid` (agent_fault): Re-send with a valid signature, token and timestamp for the verify config.
- `signature_mismatch` (agent_fault): Re-send with a signature that matches the verify config.
- `signup_paused` (host_refusal): Retry after data.retry_after_secs; signup is paused.
- `signup_rate_limited` (host_refusal): Retry after data.retry_after_s; too many tenants were created from this address.
- `spec_not_exposed` (host_refusal): Ask the owner to expose the spec; it is not shared.
- `spec_too_large` (plan_limit): Reduce the spec below the size cap, for example by moving data to host.blob.put.
- `state_quota` (plan_limit): Free state space or move to a plan with a larger state quota.
- `state_quota_exceeded` (plan_limit): Delete state rows or move to a plan with a higher state_bytes_max.
- `state_table_not_found` (agent_fault): Declare the table first or use a declared name.
- `state_unavailable` (host_refusal): This call context has no state backend; call the tool directly instead of from a sandbox.
- `step_tool_not_allowed` (agent_fault): Use a host step from the allowlist named in the message.
- `step_tool_not_found` (agent_fault): Use a tool that exists; data.suggestions lists near matches.
- `storage` (host_fault): Retry once; if it persists, report data.request_id through the support link.
- `table_already_exists` (agent_fault): Use a different table name or drop the existing one.
- `table_bound_exceeded` (agent_fault): Narrow the query so it stays under data.bound's limit.
- `table_column_not_found` (agent_fault): Use a declared column; data.candidates lists the closest ones in data.table.
- `table_not_found` (agent_fault): Declare the table first or use a declared name.
- `table_query_rejected` (agent_fault): Fix the query; data.reason says why it was rejected.
- `table_row_conflict` (agent_fault): Change the conflicting value in data.column or query the table first; nothing was appended.
- `table_unavailable` (host_refusal): This call context has no table backend; call the tool directly instead of from a sandbox.
- `task_already_claimed` (agent_fault): Pick another task; this one is already claimed.
- `task_closed` (agent_fault): The task takes no further claim or completion; post a new task.
- `task_not_claimed` (agent_fault): Claim the task with host.task.claim before completing it.
- `task_not_found` (agent_fault): List tasks and use an id that exists.
- `template_error` (agent_fault): Fix the template in the field named in data.field.
- `tenant_disabled` (host_refusal): Read data.disabled_reason. self_offboard: this account was closed by self_offboard; call any host.* with no key on a new session (or signup) to start fresh. admin_disable: do not retry.
- `tenant_key_invalid` (agent_fault): Call signup for a fresh key; this tenant_key matched no tenant.
- `tenant_key_missing` (agent_fault): Pass tenant_key, or reconnect through your onboarding url; data.tenant names the tenant when known.
- `tenant_key_required` (host_refusal): Call with the tenant key only; this tool refuses an end-user identity.
- `tenant_not_empty` (agent_fault): Import into a new empty tenant; this one already holds data.
- `tenant_not_found` (agent_fault): Check the namespace spelling and use a tenant that exists.
- `thread_not_found` (agent_fault): List threads and use an id that exists.
- `timeout` (host_fault): The host timed out; retry shortly.
- `timeout_clamped` (plan_limit): Lower timeout_s to data.ceiling (the plan's sandbox_cpu_s_max) or upgrade the plan; calls already run with the ceiling.
- `too_large` (plan_limit): Shrink the payload to the limit in data.max_bytes.
- `too_many_invocations` (agent_fault): Send at most data.limit invocations per call.
- `too_many_proposals` (plan_limit): Wait for an open proposal to be decided; data.limit is the maximum.
- `too_many_rows` (plan_limit): Narrow since/until so the export holds fewer rows than data.max_rows.
- `too_many_tools` (plan_limit): Remove a tool or move to a plan with a higher tool limit.
- `tool_building` (host_fault): Retry shortly; the tool's environment is still building.
- `tool_exception` (agent_fault): Read data.exception_class and data.line, fix the tool's code, republish and re-run host.tool_test.
- `tool_missing` (agent_fault): Restore the target tool or remove the trigger.
- `tool_not_found` (agent_fault): Check the name against host.tool_list; shared tools are called as <owner_namespace>.<tool>.
- `tool_oom` (agent_fault): Reduce the tool's memory use or the input size.
- `tool_output_invalid` (agent_fault): Return JSON that matches the tool's declared output.
- `tool_output_too_large` (agent_fault): Return less data; the output exceeds the cap.
- `tool_process_limit` (agent_fault): Reduce the number of processes or threads the tool starts.
- `tool_run_unsupported` (host_refusal): This kind cannot be run this way; call the tool through host.tool_call.
- `tool_timeout` (agent_fault): Make the tool finish within the time cap, or reduce its input.
- `tool_trapped` (agent_fault): The tool trapped; fix the runtime fault in its code and republish.
- `trace_not_found` (agent_fault): Use a call_id from a recent call; traces expire.
- `transaction_nested` (host_fault): Retrying fails the same way; report data.request_id through the support link.
- `trigger_interval_too_short` (plan_limit): Use a schedule at or above the plan's schedule_min_interval_s.
- `trigger_invalid` (agent_fault): Fix the trigger field named in data.field.
- `trigger_kind_mismatch` (agent_fault): Remove the existing trigger of the other kind before reusing the name.
- `trigger_not_found` (agent_fault): List triggers and use an id that exists.
- `trigger_quota_exceeded` (plan_limit): Remove a trigger or move to a plan with a higher event_triggers_max.
- `typescript_not_supported` (agent_fault): Strip the types and publish plain JavaScript (ES2020); data.line is the first line with TypeScript syntax.
- `unknown_argument` (agent_fault): Remove the argument; data.known lists the recognised ones.
- `unknown_import` (agent_fault): Use a module from the hint in data.hint; the import is not available in the sandbox.
- `unknown_kind` (agent_fault): Choose a kind from data.registered; data.did_you_mean suggests a near match.
- `unknown_spec_field` (agent_fault): Remove the spec field or use one the kind documents.
- `untested_tool_cannot_be_public` (agent_fault): Read data.verdict and data.detail, fix the tool, pass host.tool_test (data.next) for its current version, then set visibility public again.
- `unverifiable` (agent_fault): Call data.call_instead for real; no dry run exists for this verb.
- `upstream_auth` (upstream_fault): The upstream rejected the credential (HTTP 401/403). If the tool sends a secret, set or rotate it with host.secret.set, then retry; otherwise reconnect the upstream.
- `upstream_client_error` (upstream_fault): Fix the request the tool sends; the upstream rejected it as a client error.
- `upstream_error` (upstream_fault): Retry later; the upstream failed with a server error.
- `upstream_not_connected` (agent_fault): Connect the upstream through the vault link first.
- `upstream_rate_limited` (upstream_fault): Wait and retry; the upstream is rate limiting this tool.
- `upstream_status` (upstream_fault): Read data.upstream_status for the upstream's HTTP status, then fix the request or retry later.
- `upstream_timeout` (upstream_fault): Retry later; the upstream did not answer in time.
- `upstream_unreachable` (upstream_fault): Retry later; the upstream could not be reached.
- `uptime_unavailable` (host_refusal): This host has no http kind registered to probe with; do not retry.
- `validation` (agent_fault): Fix the counter or field named in data.key.
- `vault_provider_not_found` (agent_fault): Register the vault provider first or use a registered name.
- `webhook_body_too_large` (plan_limit): Send a body under data.limit_bytes.
- `webhook_not_found` (agent_fault): Use a webhook that exists.
- `webhook_over_quota` (plan_limit): Retry after the daily reset; the tenant is over its calls_per_day quota.
- `webhook_row_not_found` (agent_fault): Use an inbox row id that exists for this webhook.
- `webhook_signature_invalid` (agent_fault): Re-send with a signature that matches the webhook secret.
- `wrong_tenant` (agent_fault): Use the credential of the tenant this URL belongs to.
<!-- mcphost-errors:end -->

<!-- mcphost-devloop:start -->
## Local loop

Work from a directory on disk before anything is published:

- `mcphost tool pack <DIR> [--as-mcp-call]` -- Print the `host.tool_publish` payload for a tool directory
- `mcphost tool unpack <PAYLOAD> <DIR>` -- Write a payload (a file, or `-` for stdin) back to a tool directory
- `mcphost tool lint <DIR>` -- Run the server's own spec validators over a tool directory
- `mcphost dev <DIR> [--no-sandbox] [--plan <PLAN>] [--port <PORT>] [--call <TOOL> <JSON>]` -- run a tool directory locally through the host's own kinds and error mappers
- `mcphost push <DIR> [--url <URL>] [--json] [--signup] [--watch]` -- publish a tool directory to a tenant
<!-- mcphost-devloop:end -->
