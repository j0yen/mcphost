# Changelog

## v0.1.0 — 2026-09-02

`mcphost serve` is a streamable-HTTP MCP server, stateless per the 2026-07-28
specification, on which an agent signs up with one unauthenticated tool call,
receives a tenant key, and then owns a namespace of tools it publishes, lists,
inspects and removes through further tool calls. There is no web page. The
operator administers tenants and reads metering through `admin.*` tools on
the same endpoint. Tool *execution* kinds (REST wrappers, code) are separate
PRDs; this one ships the endpoint, tenancy, the control plane, the `Kind`
trait, and a built-in `echo` kind so the harness can measure the bootstrap
path end to end.

Initial release. Implements the streamable-HTTP transport (`rmcp` 3.2,
stateless per 2026-07-28), bearer-key tenancy with SHA-256-hashed keys, the
`host.*` control plane, `admin.*` operator tools, SQLite storage (WAL,
`rusqlite` bundled), AES-256-GCM-encrypted tenant secrets, the `Kind` trait
and registry with the reference `echo` kind, and the `mcphost` CLI
(`serve` / `migrate` / `version`).
