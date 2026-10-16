# Wave 1 listings checklist — directory submissions

Prepared 2026-09-30 on `chore/listings-wave1`. This file is read-only prep:
nothing here was submitted to any external site. Joe submits each listing
by hand, outward-facing, per the market-test plan
(`visions/mcphost-market-test.md`) and the readiness rule below.

## Readiness gate (wave 1)

Per `~/Notes/wiki/decisions/2026-10-01-mcphost-traffic-wave-readiness-rule.md`:

> **Wave 1 (directory listings: glama, mcp.so, pulsemcp, smithery):** one
> green customer-journey run on the binary serving prod. Joe submits
> listings (outward-facing).

Confirm one green journey run before submitting any form below. The rule's
config path (`~/.config/synthorg/readiness.toml`,
PRD-synthorg-mcphost-market-test-live-proof requirement 7) does not exist
on orch yet as of 2026-09-30 — that PRD is still queued, so the gate must
be checked by hand (re-run the journey, confirm green) until it lands.

## Per-directory

| Directory | Submission form | Account needed | Fields to paste | Review time | Verify after |
|---|---|---|---|---|---|
| **Glama** | https://glama.ai/mcp/servers | Likely none — Glama auto-indexes public GitHub repos carrying an `mcp`/`mcp-server` topic. `j0yen/mcphost`'s topics already include `mcp`, `mcp-host`, `mcp-server` (confirmed via `gh api repos/j0yen/mcphost --jq .topics`, 2026-09-30), so the precondition is met. Only sign in (GitHub OAuth) if the auto-index hasn't picked it up and a manual claim form appears. | `deploy/listings/forms/glama.md` → Payload + descriptions | Unknown (auto-index; no stated SLA) | `curl https://glama.ai/mcp/servers/@j0yen/mcphost` (or the real resolved slug) returns `mcphost.dev` in the body |
| **Smithery** | https://smithery.ai | Yes — GitHub-connect flow as of the site's current onboarding (confirm at submission time; mechanism changes). **Needs Joe's GitHub login (j0yen).** | `deploy/listings/forms/smithery.md` → Payload + descriptions | Unknown (confirm at submission) | `curl https://smithery.ai/server/j0yen/mcphost` (or real slug) returns `mcphost.dev` in the body |
| **mcp.so** | https://mcp.so (submit-server flow) | Yes, likely GitHub or email signup (confirm in browser — exact flow not verified in this prep pass). **Needs Joe's login.** | `deploy/listings/forms/mcpso.md` → Payload + descriptions | Unknown (confirm at submission) | `curl https://mcp.so/server/mcphost/j0yen` (or real slug) returns `mcphost` in the body |
| **PulseMCP** | https://www.pulsemcp.com | Yes, likely GitHub or email signup (confirm in browser — site returns 403 to scripted probes, so this prep pass could not inspect the form itself). **Needs Joe's login.** | `deploy/listings/forms/pulsemcp.md` → Payload + descriptions | Unknown (confirm at submission) | `curl https://www.pulsemcp.com/servers/j0yen-mcphost` (or real slug) returns `mcphost` in the body |
| **awesome-mcp-servers (punkpeye)** | PR against https://github.com/punkpeye/awesome-mcp-servers | Yes — GitHub account to fork + open a PR. **Needs Joe's GitHub login (j0yen).** | `deploy/listings/prs/awesome-mcp-servers-punkpeye.md` → one-line entry + process steps | Days to weeks (community-reviewed PR) | `curl https://raw.githubusercontent.com/punkpeye/awesome-mcp-servers/main/README.md \| grep j0yen/mcphost` |
| **awesome-mcp (wong2)** | PR against https://github.com/wong2/awesome-mcp-servers | Yes — same as above. **Needs Joe's GitHub login (j0yen).** | `deploy/listings/prs/awesome-mcp-wong2.md` → one-line entry + process | Days to weeks | `curl https://raw.githubusercontent.com/wong2/awesome-mcp-servers/main/README.md \| grep j0yen/mcphost` |
| **Official MCP Registry** | Not a wave-1 submission task — already published (`submitted: "2026-09-06"`), but stale (`server.json` showed 0.14.0 against repo's actual 0.64.0). Fixed in this PR: `server.json`'s `version` field now reads `0.64.0`. | N/A — republished by `mcphost-deploy`'s redeploy reconcile job (PRD-mcphost-listing-freshness), not submitted by hand. | N/A | Reconcile runs on redeploy | `curl https://registry.modelcontextprotocol.io/v0/servers?search=mcphost` and check `version` reads `0.64.0` once the reconcile job next runs — it has not run as part of this change |

## After each submission

Run `scripts/listings-check.sh` from the repo root — it fetches every
`deploy/listings/manifest.yaml` entry's `check_url` and reports
`listed` / `pending-review` / `missing` / `stale` per directory, using
each row's `search_text` (not a version check for the four form-based
directories — only the registry entry is version-checked). Record the
real submission date into `manifest.yaml`'s `submitted` field via
`scripts/listings-submit.sh` (requires `PUBLISH-OK`, human-submitted —
see that script's own header comment; this repo holds no credentials for
any of these four sites or the two awesome-list forks).

## What this PR did NOT do

- No form was submitted, no account was created, no PR was opened against
  any third-party repo, no `PUBLISH-OK` file was added.
- The official MCP Registry was not republished — only the in-repo
  artifact (`server.json`) that the separate reconcile job reads was
  corrected.
- Directory URL slugs in `manifest.yaml` remain guesses pending real
  submission, per that file's own header comment — unchanged by this PR.
