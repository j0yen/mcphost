# PRD: mcphost-claude-code-plugin-and-snippets — one line to connect from every major client, and a Claude Code plugin

- Status: queued
- Lane: orch 2026-09-24T06:34:08.770713641+00:00 run=170
- build_target: shell
- build_into: /home/jsy/wintermute/mcphost
- build_priority: low
- publish: j0yen/private
- Vision: visions/mcphost-market-test.md
- Cited-tree: mcphost@609f122903757fd85ee23e6f6c6bba4135b59344
- Depends-on: PRD-mcphost-human-claim-magic-link.md
- Loop: mcphost-buildloop: satisfaction — signup-to-first-call from a cold client
- PM: Joe
- Drafted: 2026-09-22
- Engineering target: mcphost (README.md, www/index.html, www/llms.txt, plugin/)

## TL;DR

The landing page and README gain a "Connect" block with one copy-paste line each for Claude Code, Codex CLI, Cursor, and Claude.ai custom connectors. A `plugin/` directory ships a valid Claude Code plugin (MCP config plus a `/mcphost` skill that walks an agent through signup, first tool, first schedule, and handing the claim link to its human). llms.txt carries the same walk-through.

## Problem statement

Research on 2026-09-23 found every major client connects to a bearer or no-auth streamable-HTTP server with one command (Claude Code `claude mcp add --transport http`, Codex `codex mcp add --url`, Cursor static-header remote, Claude.ai custom connectors), yet README.md on origin/main mentions none of them and no plugin or skill exists (grep for `claude mcp add|codex|cursor|connector|plugin|skill` returns nothing; no `plugin/` or `.claude` paths). Direct-install is the highest-intent channel in the plan (§4) and the plugin marketplaces are the one place Claude Code users browse; without these assets the launch sends visitors to a page that does not tell them how to connect.

## Goals

- A visitor connects from any of the four clients in under a minute by copying one line.
- A Claude Code user installs the plugin and the `/mcphost` skill drives the agent through the first-run story ending in the claim link.

## Non-goals

- Submission to the official Anthropic marketplace or Connectors Directory (needs a Team/Enterprise org; a later task).
- OAuth.
- A VS Code or JetBrains extension.

## User stories

- Agent operator on Claude Code: I paste one line, my agent has mcphost; I say "give yourself a nightly job" and it does.
- Agent operator on Codex: same, with `codex mcp add`.
- Cursor user: the docs give me the JSON block with the bearer header placeholder.
- Claude.ai user: the docs tell me where custom connectors live and what URL to paste.
- Marketplace browser: I find `mcphost` in a community marketplace, install, and the skill explains the claim step.

## Requirements

P0
1. `README.md` gains a `## Connect` section with four fenced one-liners or blocks (Claude Code, Codex CLI, Cursor `mcp.json`, Claude.ai custom connector steps), each containing `https://mcphost.dev/mcp`.
2. `www/index.html` shows the same four snippets in a tabbed or stacked block above the fold on desktop and within the first screen on mobile, with a copy button that needs no external script.
3. `plugin/.claude-plugin/plugin.json` with name `mcphost`, version matching Cargo.toml, description, author `j0yen`; `plugin/.mcp.json` declaring the `mcphost` server at `https://mcphost.dev/mcp` (transport http); `plugin/skills/mcphost/SKILL.md` with frontmatter name/description and a body that walks: signup (handoff mode, `source: "plugin"`), redeem, publish one tool, set one schedule, relay `claim_url` to the human, never print the key.
4. `www/llms.txt` gains the same first-run walk-through and the claim relay sentence (from the claim PRD) so an agent reading the site alone reaches the same outcome.
5. A test script `tests/plugin_assets.sh` asserts: all files exist, both JSON files parse with `jq`, plugin version equals Cargo.toml version, every snippet contains the URL, SKILL.md contains `claim_url` and `source: "plugin"`, and no snippet contains a real key.

P1
6. `claude plugin validate plugin/` exits 0 when the `claude` CLI is present on the build host; when absent the test records `skipped` in the receipt rather than failing.
7. README "Connect" links to the status page and the AUP.

P2
8. A 90-second asciinema cast of the first-run story checked in under `www/demo/` (recorded by the operator; the PRD only reserves the path and links it if present).

## Success metrics

| Metric | Baseline | Target | Method | Timeframe |
|---|---|---|---|---|
| Signups with `source: "plugin"` or `"docs"` | 0 | ≥ 10 | admin healthz signups_by_source | 4 weeks after hold lifts |
| Cold-client signup-to-first-call, median | 30.7 s (synthetic panel) | < 60 s for external | measure loop | same |
| Guardrail: snippet drift from live URL | n/a | 0 | AC5 in CI | continuous |

## Technical considerations

- The plugin is static files; no build step. Keep it in the mcphost repo so the version test can pin it to Cargo.toml.
- The www copy button uses inline JS only (the site has no bundler).
- Client syntax to verify at build time against current docs: `claude mcp add --transport http mcphost https://mcphost.dev/mcp`; `codex mcp add mcphost --url https://mcphost.dev/mcp`; Cursor `"mcphost": {"url": "https://mcphost.dev/mcp", "headers": {"Authorization": "Bearer <key>"}}`.

## Migration / compatibility

None. Additive files and docs.

## Open questions

| Question | Owner | Due |
|---|---|---|
| Which community marketplace first (buildwithclaude or another)? | Joe | before Wave A |

## Acceptance criteria

1. P0 — Given the built tree, When `tests/plugin_assets.sh` runs, Then it exits 0 and prints one line per check.
2. P0 — Given `plugin/.claude-plugin/plugin.json` and `plugin/.mcp.json`, When parsed with `jq`, Then both succeed, `.name == "mcphost"`, and `.version` equals the `version` in Cargo.toml.
3. P0 — Given `README.md`, When grepped, Then a `## Connect` heading exists and `https://mcphost.dev/mcp` appears at least four times below it, once each within blocks mentioning Claude Code, Codex, Cursor, and Claude.ai.
4. P0 — Given `www/index.html`, When grepped, Then the four snippets are present and the page loads with no external script tags added by this change.
5. P0 — Given `plugin/skills/mcphost/SKILL.md`, When grepped, Then it contains `handoff`, `redeem`, `source: "plugin"`, `claim_url`, and the phrase "never print the key".
6. P0 — Given `www/llms.txt`, When grepped, Then it contains the first-run walk-through headings (signup, publish, schedule, claim) in that order.
7. P0 — Given any file under `plugin/` or the snippets, When grepped for a 40+ character base64-like token following `Bearer `, Then only the literal `<key>` placeholder appears.
8. P1 — Given the `claude` CLI on PATH, When `claude plugin validate plugin/` runs, Then exit 0; Given it is absent, Then the test prints `skipped: claude cli absent` and exits 0.
9. P1 — Given README "Connect", When grepped, Then it links to `/status.html` and `/aup.html`.
