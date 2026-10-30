# Sandbox bridge: `mcphost.elicit`

`mcphost.elicit(message, schema, timeout_s=None)` asks the user a typed
question while a python tool runs. When the calling client advertised form
elicitation on a connection that can carry a server-to-client request, the
host sends `elicitation/create` and returns the client's answer; every other
caller gets an ask-user question (`host.ask.create`) and waits for the signed
page's answer, or receives `{action: "pending", ask_id, url, expires_unix}`.

The accepted schema is one flat object whose properties are primitive types.
This table is generated from `ELICIT_SCHEMA_SUBSET` in `src/elicit.rs`; a test
asserts the bridge docstring (`help(mcphost.elicit)`) and the rmcp mapping list
exactly the same types and limits.

<!-- mcphost-elicit-subset:start -->
| rule | value |
|---|---|
| types | string, number, integer, boolean, enum |
| max properties | 8 |
| string maxLength | 1000 |
| enum values | 12 |
| elicits per call | 3 |
<!-- mcphost-elicit-subset:end -->

Results: `{action: "accept", content}`, `{action: "decline"}`,
`{action: "cancel"}` or `{action: "pending", ask_id, url, expires_unix}`.
Errors (raised as `mcphost.elicit.ElicitError`, carrying `.code` and `.data`):
`elicit_schema_invalid {path, allowed}`, `elicit_timeout`,
`elicit_unsupported {reason}`, `elicit_calls_exceeded {limit}`.

# Sandbox bridge: `mcphost.sampling`

`mcphost.sampling.ask(prompt, max_tokens=256, system=None, hints=None)` asks
the calling client's own model one question while a python tool runs. The host
sends `sampling/createMessage` on the calling connection when that connection
advertised `sampling` at `initialize`, and returns
`{text, model, stop_reason, tokens}`. The cost is the client's, not a provider
key's: nothing is sent to any provider the tenant configured.

Every other caller gets `sampling_unsupported {reason, alternative, help_url}`
at once, never a hang: `reason: "client"` when the live connection did not
advertise `sampling`, `reason: "no_client"` for a job or trigger run (which has
no connection). `alternative` names the prompt kind with the tenant's own key.

Each ask has a 30 s wall: a client that has not answered by then fails the ask
`sampling_timeout {asked: true}`. An ask made with less than 30 s of the tool's
own `timeout_s` left fails at once `sampling_timeout {remaining_ms, asked:
false}` without asking the client, so a tool that samples declares
`timeout_s` of at least 31.

Sampled tokens are metered per tenant against the plan's
`sampling_tokens_per_day` (free 20 000, pro 500 000; UTC day), and
`host.usage` reports `sampling_tokens_today`. An ask is admitted on an estimate
of `max_tokens + len(prompt)/4`; what is then recorded is the client's own
usage report (`_meta.usage`) when it sends one, else `(len(prompt) +
len(reply))/4`. An ask that would pass the ceiling is refused before the
client is asked.

`host.tool.logs` shows each ask as one counts-only line,
`{"sampling": {"asks", "tokens", "prompt_len", "reply_len"}}` (a refusal carries
`"error": <code>` instead of the lengths); the prompt and the answer are never
logged.

Errors (raised as `mcphost.sampling.SamplingError`, carrying `.code` and
`.data`): `sampling_unsupported {reason, alternative, help_url}`,
`sampling_timeout {remaining_ms, wall_ms, asked}`,
`sampling_calls_exceeded {limit}` (at most 4 asks per call; the 5th is refused
before the client is asked), `sampling_quota_exceeded {limit, used,
reset_unix}`.

# Sandbox bridge modules

Every `mcphost.*` module a python tool can import. This block is generated from
`BRIDGE_MODULES` in `src/kinds/python.rs`, the same constant the
`unknown_import` hint and `host.quickstart` read; a test asserts they list
exactly the same modules.

<!-- mcphost-bridge-modules:start -->
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
- `mcphost.table.TableError` -- `.code` is one of `table_already_exists`, `table_bound_exceeded`, `table_column_not_found`, `table_not_found`, `table_query_rejected`, `table_row_conflict`, `table_unavailable`; `.data` carries the details (e.g. `data.column` on `table_row_conflict`)
<!-- mcphost-bridge-modules:end -->
