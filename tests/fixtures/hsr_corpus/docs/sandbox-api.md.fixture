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
