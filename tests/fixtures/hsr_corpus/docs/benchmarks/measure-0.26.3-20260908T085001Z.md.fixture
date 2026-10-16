# Measure run 0.26.3-20260908T085001Z

Mirrored from `evidence/mcp-host/measure/0.26.3-20260908T085001Z/ledger.jsonl`
(private `j0yen/prds` evidence repo, not shipped with this crate) so the
public README and `llms.txt` can cite a number that also exists inside
`j0yen/mcphost` itself — `scripts/copy-claims.sh` checks against this file,
not the private evidence repo, so the claim audit does not depend on a repo
that CI never checks out.

- `deployed_version`: `0.26.3`
- panel: 21 sessions, 7 buyer segments (`admin_agent`, `cost_optimizer`,
  `data_pipeline_builder`, `integration_specialist`, `rag_indexer`,
  `rapid_prototyper`, `workflow_orchestrator`), `cold`/`warm` conditions
- client: `claude-sdk` 2.1.251

| metric | n | median | unit |
|---|---|---|---|
| `t_first_publish` (signup → first successful `host.tool_publish`) | 21 | 30.72 | seconds |
| `t_first_own_call` (signup → first successful call on a tenant's own published tool) | 20 (1 session's tool never finished building; no `t_first_own_call`) | 42.40 | seconds |

Recomputed 2026-09-08 (PRD-mcphost-agent-findability build) directly from
the ledger's `t_first_publish` / `t_first_own_call` fields with a plain
median (sorted values, midpoint of the two central values on an even
count):

```
$ python3 -c "
import json
pub, call = [], []
for line in open('ledger.jsonl'):
    line = line.strip()
    if not line: continue
    d = json.loads(line)
    if d.get('t_first_publish') is not None: pub.append(d['t_first_publish'])
    if d.get('t_first_own_call') is not None: call.append(d['t_first_own_call'])
def median(xs):
    xs = sorted(xs); n = len(xs)
    return xs[n // 2] if n % 2 else (xs[n // 2 - 1] + xs[n // 2]) / 2
print('t_first_publish', len(pub), median(pub))
print('t_first_own_call', len(call), median(call))
"
t_first_publish 21 30.72465760691557
t_first_own_call 20 42.40137840353418
```

The problem statement's earlier figures (~23.6s / ~31.2s) came from an
older run; this file always names the run id it was mirrored from so a
future PRD updating the README also updates this file — `copy-claims.sh`
fails the build if the two drift apart from the citation without a
matching numeric mirror.
