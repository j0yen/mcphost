# Tool naming rule

PRD-mcphost-tool-naming-convention-and-aliases. One rule, enforced by
`scripts/tool-naming-lint.sh` and `tests/toolname_ac01_lint_catches_new_violations.rs`,
so an agent that knows one `host.*` tool's name can predict the next one.

## The rule

Every tool under the `host.` namespace is named `host.<family>.<verb>`:

- `<family>` is a noun (`tool`, `key`, `secret`, `self`, `docs`, `agent`,
  `group`, `msg`, `table`, `state`, `lineage`, `trigger`, `runs`,
  `catalog`, `channel`, `oauth`, `enduser`, `share`, `registry`, `bridge`,
  `spec`, ...) — lower-case letters and digits only, never an underscore.
- `<verb>` is a single word (`get`, `list`, `set`) or `verb_object`
  (`spec_shared`, `delete_rows`, `table_create`) — underscores inside the
  verb segment are fine; it's the family/verb *separator* that must be a
  dot, never an underscore.

## Exceptions

| Exception | Examples | Reason |
|---|---|---|
| Singleton nouns | `host.whoami`, `host.redeem`, `host.quickstart`, `host.usage`, `host.changelog`, `host.export`, `host.progress` | A tool that is itself one word, with no sibling verb registered under that name, keeps `host.<noun>` rather than a trivial `host.<noun>.<noun>` split. See `tool_aliases::SINGLETON_NOUN_EXCEPTIONS`. |
| `signup` | — | Documented top-level exception: predates the `host.` namespace and is the first word every client learns. |
| `billing.*` | `billing.plans`, `billing.status`, `billing.checkout` | Documented top-level family exception, its own namespace. |
| `admin.*` | — | Operator-only surface. A separate naming rule entirely — out of scope for this PRD and this lint. |

## Aliases

Every tool name that violated this rule as of 2026-10-01 keeps working,
unchanged, as a deprecated alias of its new canonical name — see
`src/tool_aliases.rs`'s `ALIASES` table (the single source every consumer
reads: the registry, `tools/list`, dispatch, and the lint). Calling an
alias:

- Dispatches to exactly the same handler as its canonical, with the same
  arguments and the same result.
- Appears in `tools/list` with `_meta["x-deprecated"] = {replaced_by,
  sunset}` (`sunset` = landing date + 90 days — see `ALIAS_SUNSET_DATE`).
- Returns that same `_meta["x-deprecated"]` object on the call result too.
- Logs `tool_deprecated_alias tenant=<hash> alias=<a> canonical=<c>` at
  most once per tenant per day.

No alias is ever removed by this PRD — a sunset date is set; removal is a
later PRD once usage reaches zero (see the alias-call counters named in
`docs/metrics.md`).

The full canonical/alias table as of landing is in `docs/tools.md`
("Renamed tools (deprecated aliases)"), generated from the same registry
`tools/list` serves.

## How to add a tool

1. Register it in `src/handler.rs`'s `host_tools()` (or `admin_tools()` for
   an operator-only tool — that surface follows its own, separate rule) as
   `host.<family>.<verb>`, or add it to `SINGLETON_NOUN_EXCEPTIONS` in
   `src/tool_aliases.rs` with a one-line reason if it's genuinely a
   standalone noun with no sibling verb.
2. Add its dispatch arm in `dispatch_tenant_tool`'s `match name { ... }`.
3. Run `scripts/tool-naming-lint.sh` — it fails loudly, naming the
   violation and (when mechanically derivable) a suggested canonical form,
   if the new name doesn't fit.
4. Run `cargo run --bin mcphost -- tools-doc` to regenerate `docs/tools.md`,
   and `cargo run --bin mcphost -- llms-txt` to regenerate `www/llms.txt`'s
   tool list — both are generated from the registry, never hand-edited for
   names.
5. Never rename an existing tool outright: add the new canonical name,
   then add `(old_name, new_name)` to `ALIASES` in `src/tool_aliases.rs`
   so the old name keeps working through its sunset.
