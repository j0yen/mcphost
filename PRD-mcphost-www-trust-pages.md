# PRD: mcphost-www-trust-pages — pricing, use cases, compare, and an agent skill file for mcphost.dev

- Status: building
- Direct-build: 2026-10-01 carbon, Joe "pages" (direct, not wm-build)
- build_target: shell
- build_into: /home/jsy/wintermute/mcphost
- build_priority: normal
- publish: j0yen/public
- Vision: visions/mcphost-market-test.md
- Loop: mcphost-buildloop: satisfaction — visitor reaches a reason to connect without leaving the site
- PM: Joe
- Drafted: 2026-09-30
- Engineering target: mcphost (www/pricing.html, www/use-cases.html, www/compare.html, www/skill.md, www/sitemap.xml, www/llms.txt, www/index.html footer); mcphost-deploy (Caddy routes in install.py, hand commit after land)

## TL;DR

mcphost.dev gains four pages the rewritten home page (branch `www/trust-funnel`) points at: `/pricing`, `/use-cases`, `/compare`, and `/skill.md`. Each page follows the pattern the 2026-09-30 site study found working on Backenly: state the problem, show the sequence mcphost runs, say what stays the reader's job, and name a limitation. The four pages are static HTML in the committed terminal look, listed in the sitemap, linked from the footer, and routed by Caddy in mcphost-deploy.

## Problem statement

The 2026-09-30 study of backenly.com (wiki answer `2026-09-30-mcphost-dev-site-vs-backenly-ia.md`) found that the reader who lands on a one-page site has nowhere to go when the home page raises a question: what it costs in their case, whether their workflow is one the product is built for, and how it differs from the product they already know. mcphost.dev serves `/`, `/docs`, `/status`, legal pages, and `llms.txt`; `/pricing`, `/about`, `/blog`, `/signup`, and `/login` return 404 (fetched 2026-09-30). Backenly answers all three questions on separate pages, each ending in a trade-off, and its use-cases page says why: a page that cannot name what it does not do is a brochure. The market-test plan's first traffic wave (listings, then Show HN) sends readers who will ask exactly these questions.

## Goals

- A visitor who wants a price, a fit check, or a comparison finds a page for it, and each page ends in the connect line.
- An agent that reads the site alone finds a skill file that teaches it the first run before it touches anything.
- Every claim on the four pages is traceable to `www/llms-full.txt`, `docs/`, or a competitor's public page, with the URL in a comment.

## Non-goals

- A blog, an about page, or a sign-up or log-in page (signup is a tool call).
- A CMS, a static-site generator, or any build step; the site stays hand-written HTML.
- Copying Backenly's page structure wholesale; the mcphost pages keep the single-column terminal layout.
- Changing prices or quotas.

## User stories

- Agent operator deciding whether to connect: I open `/use-cases`, find the workflow that matches mine, and read what it will not do before I spend an hour.
- SaaS builder evaluating per-end-user auth: I open `/compare`, see in one table that Backenly and Smithery do not do this, and go to `/docs` for the OAuth section.
- Buyer with a budget question: I open `/pricing`, see what a call is, what is never metered, and how I leave.
- Coding agent reading the site: I fetch `/skill.md`, learn signup, first tool, first schedule, and the claim handoff, and only then connect.

## Requirements

P0
1. `www/pricing.html`: the two plans as on the home page (Free early access: 50 tools, 500 calls a day, no card; Pro $19 a month, 50,000 calls, then $0.001 a call), a quota table copied from `www/llms.txt` (tools per tenant, spec size, result size, call timeout, signup rate limit), one paragraph on what is metered (calls) and what is not (logs, secrets, schedules, status), one paragraph on billing by tool call (`billing.plans`, `billing.status`, `billing.checkout`), the early-tenant promise, and the exit path (plain MCP, change the endpoint, run the source).
2. `www/use-cases.html`: three workflows, each with five labelled parts: the problem in the reader's words, what they would otherwise build, the sequence mcphost runs as a tool-call listing, what stays their job, and one limitation. Workflows: (a) a coding agent that must keep working unattended gives itself a nightly job; (b) a SaaS backend where each call acts as a different end user over OAuth; (c) a fleet of agents sharing tools and messaging through the inbox. Every tool name in a listing must appear in `www/llms-full.txt`.
3. `www/compare.html`: a table with rows for mcphost, Backenly, Smithery and Klavis, and Cloudflare Agents, and columns: what it hosts, who provisions the account, per-end-user identity on a call, open source licence, free tier, exit path. Competitor cells are concrete feature facts with a source URL in an HTML comment; a cell the builder cannot source becomes "not stated" rather than a guess. Below the table, one paragraph per competitor on when to choose it over mcphost.
4. `www/skill.md`: the agent skill. If `plugin/skills/mcphost/SKILL.md` exists on the tree, `www/skill.md` is byte-identical to it and a test asserts that; if it does not, write it to the plugin PRD's spec (signup in handoff mode, redeem, publish one tool, set one schedule, relay `claim_url` to the human, never print the key) and the test asserts those phrases.
5. `www/sitemap.xml` lists the four new paths; `www/llms.txt` links `/skill.md` in its first ten lines; the footer of `www/index.html` gains `pricing · use cases · compare · skill.md` before the legal links.
6. All four HTML pages reuse the home page's inline CSS (same palette, `## ` headings, `.listing` blocks), load no external script or stylesheet, carry `<title>`, `<meta name="description">`, and `og:title`/`og:description`, and end with the connect line and the Claude Code one-liner.
7. `agent/proof-lanes.toml`: confirm the existing `www/**` lane covers the new files; if no lane matches, add one in the same change (per `www/llms-full.txt` "Contributing: routing a new top-level path").

P1
8. Caddy routes in `mcphost-deploy/src/mcphost_deploy/install.py` for `/pricing`, `/use-cases`, `/compare` (rewrite to the `.html` file, as `/aup` is done) and `/skill.md` as a plain file; the `www content` verdict line in `cli.py` lists the new paths. This is a hand commit in the deploy repo after the mcphost change lands, tagged in the landing note.
9. `tests/www_pages.sh`: asserts the four files exist, each HTML page parses with Python's `html.parser`, contains the connect line, contains no `<script src=` or `<link rel="stylesheet"`, and every `host.*`, `billing.*`, and `mcphost.*` identifier in `use-cases.html` and `pricing.html` appears in `www/llms-full.txt`.

P2
10. `/compare` gains a row for InsForge once its public docs state the six columns.

## Success metrics

| Metric | Baseline | Target | Method | Timeframe |
|---|---|---|---|---|
| Visits that reach a second page | unknown (no analytics yet) | ≥ 30% of home visits | Caddy access log, path counts | 4 weeks after wave 1 |
| Signups whose first `tools/list` follows a `/skill.md` fetch from the same IP within 10 min | 0 | ≥ 5 | Caddy log join | same |
| Guardrail: unsourced competitor claim | n/a | 0 | AC3 review | at land |

## Technical considerations

- Static files only; the site has no bundler. Copy the CSS block from `www/index.html` rather than linking a shared stylesheet, so each page stays self-contained as the existing pages are.
- Competitor facts as of 2026-09-30 are in the wiki source `sources/backenly-competitive-dossier.md` (Backenly: Postgres, REST, auth, storage, realtime over MCP; Apache-2.0 platform; Free $0 and Pro $25 a month; no per-end-user identity on a call; human creates the project and key). Re-verify each against the live page at build time.
- The timeline on the home page dropped inbound webhooks because `llms.txt` does not document them; the use-case listings must not reintroduce them unless the docs do.
- `www/skill.md` is served by Caddy as `text/markdown`; confirm the content type in the route.

## Migration / compatibility

Additive. The home page footer change touches `www/index.html`, which branch `www/trust-funnel` also edits; build this PRD from that branch or after it lands to avoid a conflicting footer edit.

## Open questions

| Question | Owner | Due |
|---|---|---|
| Does `/compare` name Backenly by name on launch day, or wait until their Product Hunt launch is public? | Joe | before wave 1 |
| Should `/skill.md` be the single source and `plugin/skills/mcphost/SKILL.md` a copy of it, or the reverse? | Joe | at build |

## Acceptance criteria

1. P0 — Given the built tree, When `tests/www_pages.sh` runs, Then it exits 0 and prints one line per check.
2. P0 — Given `www/pricing.html`, When grepped, Then it contains `$19`, `500 calls`, `50,000`, `$0.001`, `billing.checkout`, and the phrase "change the endpoint".
3. P0 — Given `www/use-cases.html`, When grepped, Then three `<h2>` headings exist and each of the five part labels (problem, otherwise, sequence, yours, limitation) appears three times, and every `host.*` or `mcphost.*` identifier in it appears in `www/llms-full.txt`.
4. P0 — Given `www/compare.html`, When parsed, Then one table has four data rows and six columns, and every non-mcphost cell is either preceded by an HTML comment containing `http` or reads "not stated".
5. P0 — Given `plugin/skills/mcphost/SKILL.md` exists, When compared with `www/skill.md`, Then `cmp` exits 0; Given it does not exist, Then `www/skill.md` contains `handoff`, `claim_url`, and "never print the key".
6. P0 — Given `www/sitemap.xml`, `www/llms.txt`, and `www/index.html`, When grepped, Then the sitemap lists `/pricing`, `/use-cases`, `/compare`, `/skill.md`; `llms.txt` links `/skill.md` within its first ten lines; and the index footer links all four.
7. P0 — Given each new HTML page, When grepped, Then `<script src=` and `<link rel="stylesheet"` are absent and `claude mcp add --transport http mcphost https://mcphost.dev/mcp` is present.
8. P0 — Given `cargo test --test lanecov_ac01_every_tracked_path_routes` on the changed tree, Then it passes.
9. P1 — Given prod after `mcphost-deploy redeploy` with the new Caddy routes, When `curl -s -o /dev/null -w "%{http_code}" https://mcphost.dev/<path>` runs for each of the four paths, Then each returns 200 and `/skill.md` returns `content-type: text/markdown` (live, operator-run after the deploy repo hand commit)
10. P1 — Given `mcphost-deploy` after the hand commit, When `mcphost-deploy install --dry-run` or the equivalent renders the Caddy site block, Then the four routes appear in the rendered output.
