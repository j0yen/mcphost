# Tool naming

One rule predicts every tool name this host registers, so an agent that
knows `host.tool.publish` exists can guess `host.tool.share` correctly
before ever calling `tools/list`.

## The rule

    host.<family>.<verb>

- `host` is the fixed control-plane prefix for every tool this host owns
  (as opposed to a tenant's own namespaced tools, `<namespace>.<tool>`).
- `<family>` is a noun: `tool`, `key`, `secret`, `self`, `docs`, `agent`,
  `group`, `msg`, `table`, `state`, `lineage`, `trigger`, `runs`,
  `catalog`, `channel`, `oauth`, `enduser`, `vault`, `share`, `bridge`,
  `spec`, `registry`.
- `<verb>` is a single word, or `verb_object` when the verb alone would be
  ambiguous (`profile_set`, `caller_limit_remove`, `spec_shared`) --
  the underscore there joins the verb to its object, not two families.

Examples: `host.tool.publish`, `host.tool.share`, `host.key.rotate`,
`host.agent.profile_set`.

## Exceptions

Two kinds of name never follow the rule above, both intentional:

**Top-level, pre-namespace names.** `signup` and `billing.*`
(`billing.plans`, `billing.status`, `billing.checkout`) predate the
`host.` namespace and are the first words every client learns -- renaming
either would break the one guarantee this rule exists to protect. Not
`admin.*`: that surface has its own, separate operator-only naming rule
(non-goal of this doc).

**Singleton nouns.** A `host.<noun>` tool with no sibling verb under that
noun -- there's nothing to disambiguate, so `host.<noun>.get` would just
be noise. Today's singleton nouns: `host.whoami`, `host.redeem`,
`host.quickstart`, `host.usage`, `host.changelog`, `host.export`,
`host.progress`. `scripts/tool-naming-lint.sh` lists these, with this same
reason, every time it runs -- see that script for the authoritative,
machine-checked list (this paragraph can drift; the lint cannot).

## Aliases

A tool whose name predates this rule keeps its old name working
indefinitely as an alias: `src/tool_aliases.rs`'s `TOOL_ALIASES` table
maps `alias -> canonical`. An alias:

- dispatches to exactly the same handler as its canonical, with the same
  arguments and the same result;
- is advertised in `tools/list` with the same `inputSchema` as its
  canonical, plus `x-deprecated: {replaced_by: "<canonical>", sunset:
  "<date>"}` (sunset = the date this alias table last changed + 90 days);
- carries a `_meta.deprecated: {replaced_by, sunset}` hint on every
  successful call's result;
- logs `tool_deprecated_alias tenant=<hash> alias=<a> canonical=<c>` at
  most once per tenant per day (`docs/metrics.md` names the matching
  counter, `tool_alias_calls{tool}`, for the sunset decision).

Aliases are never removed by this PRD -- only added. Removing one (after
usage reaches zero past its sunset) is a later PRD's job.

## Adding a new tool

1. Name it `host.<family>.<verb>` (or `<existing-family>.<new-verb>` if
   it belongs to a family above). Don't invent a new singleton noun --
   if it's the first tool under a brand-new family, it still gets a verb
   (a verb from day one, not a bare noun), so that family has room to
   grow without a rename later.
2. Register it in `src/handler.rs`'s `host_tools()`/`admin_tools()`, same
   as every existing tool.
3. Run `scripts/tool-naming-lint.sh`. A violation names the rule it broke
   and the canonical name to use instead.
4. Run `scripts/gen-docs.sh` (or the specific `gen-*` script that owns the
   doc you touched) so `docs/tools.md` and friends stay generated, never
   hand-edited for names.

## CI

`scripts/tool-naming-lint.sh` runs in CI (wired into
`scripts/risk-gate.sh`) against `src/`. It exits 0 and prints
`violations=0 aliases=<n>` when every `host.*`/`billing.*`/`signup`
literal either follows the rule, is a documented exception (printed with
its reason), or is a registered alias; exits 1, naming the offending
name and its canonical form, otherwise.
