# Receipt — mcphost-docs-qa-recipe AC9: docs-qa.sh against prod after the v0.60.34 deploy

AC9: Given prod after land, When `docs-qa.sh https://mcphost.dev` runs from carbon with a fresh signup, Then it exits 0 in under 60 s and the receipt is committed under `docs/receipts/`.

## Prod state at the run

- Landed: PR #71 (squash aedb4e5), tag v0.60.34, deploy-on-land.
- `/opt/mcphost/bin/mcphost --version` on mcphost-1: `mcphost 0.60.34`; service ActiveEnterTimestamp 2026-09-28 03:06:45 UTC (8:06 pm PDT).

## What was run (carbon, 2026-09-27 8:13:29 pm PDT)

```
examples/docs-qa/docs-qa.sh https://mcphost.dev/mcp --receipt-dir <tmp>
```

Fresh signup (tenant `t_880aa34a`), the 8-document corpus put and indexed, `ask_docs` published, the 6 gold questions asked. Calls, in order: signup, host.docs.put, host.docs.status, host.tool_publish, t_880aa34a.ask_docs.

## Result

| field | value |
|---|---|
| exit code | 0 |
| wall time | 21232 ms |
| index ready | 9.215463041968178 s |
| hits | 6/6 |
| quota | {"documents": 8, "chunks": 35, "tool_calls": 6} |

Per question:

| # | question | result | citation |
|---|---|---|---|
| 1 | How many business days does a new hire have to complete onboarding paperwork? | hit | onboarding.md:617 |
| 2 | What is the meal reimbursement limit that requires an itemized receipt? | hit | expense-policy.md:557 |
| 3 | What port does the VPN client connect on? | hit | vpn-setup.md:2083 |
| 4 | Who gets paged first for a Sev1 incident? | hit | incident-response.md:0 |
| 5 | How many PTO days do employees accrue per year? | hit | pto-policy.md:1725 |
| 6 | How often must employees rotate their password? | hit | security-basics.md:0 |

Full receipt JSON (per-step timings, answers with cited passages): `docs/receipts/mcphost-docs-qa-recipe-ac9-20260927.json`.
