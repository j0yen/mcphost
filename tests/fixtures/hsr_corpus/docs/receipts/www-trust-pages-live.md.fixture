# Receipt: www-trust-pages-live

Live proof that the four trust pages from PRD-mcphost-www-trust-pages (merge b168503, #92) are served by https://mcphost.dev.

- Checked: 2026-10-01T09:06:54Z (2026-10-01 02:06 PDT) from carbon, plain curl.
- pricing 200 text/html; charset=utf-8
- use-cases 200 text/html; charset=utf-8
- compare 200 text/html; charset=utf-8
- plans 200 text/html; charset=utf-8
- skill.md 200 text/markdown
- Deploy path: mcphost-deploy routes 857edc7 (#8) + /plans 06fde6f, vendor-www e93fd77 at b168503, `redeploy` from the orch checkout (verified backup 20261001T090549Z-aee6c503d30b, probe green, "www content: written").
- Reproduce: `for p in pricing use-cases compare plans skill.md; do curl -s -o /dev/null -w "$p %{http_code}\n" https://mcphost.dev/$p; done`
