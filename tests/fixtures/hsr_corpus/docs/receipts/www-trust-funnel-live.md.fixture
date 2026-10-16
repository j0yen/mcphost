# Receipt: www-trust-funnel-live.md

Live proof that the index v2 rewrite (PR #87, merge 8a4a88a) is what https://mcphost.dev serves.

- Checked: 2026-10-01T07:55:14Z (2026-10-01 00:55 PDT) from carbon, plain curl, no cache headers.
- Live `/` md5 (first 12): 5ff678e38c6c; `origin/main:www/index.html` md5: 5ff678e38c6c. (Equal means byte-identical; a difference is expected only if Caddy rewrites nothing and the vendored copy matches, so note which.)
- Marker sentence "publishes the missing tool with a second" on the live page: 1 occurrence.
- Live h2 order: connect|one night on mcphost|what your agent can give itself|what keeps it safe|for the human|built to be depended on|before you trust it with production|pricing|source and contact.
- `/healthz`: {"ok":true}; status.json version: (empty).
- Deploy path: vendor-www 5219359 in mcphost-deploy → `redeploy` from the checkout on orch (`uv run`, backup 20261001T073743Z-aee6c503d30b, probe green, "www content: written"). The durable venv's `release` gate timed out at 900 s; see the deploy memo in the deploy repo.
- Reproduce: `curl -s https://mcphost.dev/ | grep -c "one night on mcphost"` → 1.
