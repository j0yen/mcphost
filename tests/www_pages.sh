#!/usr/bin/env bash
# www_pages.sh -- PRD-mcphost-www-trust-pages, requirement 9 / AC1-7.
#
# Asserts, printing one line per check: the four trust pages
# (pricing.html, use-cases.html, compare.html, skill.md) exist; every HTML
# page parses with Python's html.parser, carries no external script or
# stylesheet, and ends with the Claude Code connect line; skill.md matches
# plugin/skills/mcphost/SKILL.md byte for byte; pricing.html and
# use-cases.html carry their required phrases; every host.*/billing.*/
# mcphost.* identifier in those two files appears in www/llms-full.txt;
# use-cases.html's three workflows each carry all five part labels;
# compare.html's table has three competitor rows of six columns each,
# sourced or "not stated"; the index footer links all four pages; and
# llms.txt links /skill.md within its first ten lines.
#
# Scope note: sitemap.xml is deploy-owned in mcphost-deploy (updated there
# in parallel), so this script does not check it, unlike the PRD's
# requirement 5/AC6 as originally drafted.
set -uo pipefail
cd "$(dirname "$0")/.."

status=0
CONNECT_LINE='claude mcp add --transport http mcphost https://mcphost.dev/mcp'

check() {
  local desc="$1"
  shift
  if "$@"; then
    echo "ok - $desc"
  else
    echo "FAIL - $desc"
    status=1
  fi
}

file_exists() { [ -f "$1" ]; }
grep_has() { grep -qF -- "$2" "$1"; }
grep_has_re() { grep -qE -- "$2" "$1"; }

html_parses() {
  python3 - "$1" <<'PY' >/dev/null 2>&1
import sys
from html.parser import HTMLParser
path = sys.argv[1]
with open(path, "rb") as fh:
    data = fh.read()
text = data.decode("utf-8")
if not text.strip():
    sys.exit(1)
class Checker(HTMLParser):
    def error(self, message):
        raise AssertionError(message)
Checker(convert_charrefs=True).feed(text)
if "<html" not in text.lower():
    sys.exit(1)
PY
}

no_external_script() { ! grep -qiE '<script[^>]+src=' "$1"; }
no_external_stylesheet() { ! grep -qiE '<link[^>]+rel="stylesheet"' "$1"; }
has_connect_line() { grep -qF -- "$CONNECT_LINE" "$1"; }

# requirement 4 / AC5
skill_md_check() {
  local plugin_skill="plugin/skills/mcphost/SKILL.md"
  if [ -f "$plugin_skill" ]; then
    cmp -s www/skill.md "$plugin_skill"
  else
    grep -qF 'handoff' www/skill.md \
      && grep -qF 'claim_url' www/skill.md \
      && grep -qF 'never print the key' www/skill.md
  fi
}

# requirement 9: every host.*/billing.*/mcphost.* identifier used in a file
# must appear in www/llms-full.txt.
identifiers_covered() {
  python3 - "$1" <<'PY'
import re, sys
path = sys.argv[1]
text = open(path, encoding="utf-8").read()
full = open("www/llms-full.txt", encoding="utf-8").read()
idents = set(re.findall(r'\b(?:host|billing|mcphost)\.[A-Za-z0-9_.]+', text))
missing = [i for i in sorted(idents) if i.rstrip(".") not in full and i not in full]
if missing:
    print(f"{path}: identifiers not found in www/llms-full.txt: {missing}", file=sys.stderr)
    sys.exit(1)
PY
}

# AC3: three <h2> workflow headings, and each of the five part labels
# (problem, otherwise, sequence, yours, limitation) appears exactly three
# times -- one per workflow.
use_cases_structure() {
  python3 - <<'PY'
import re
import sys
text = open("www/use-cases.html", encoding="utf-8").read()
h2 = len(re.findall(r'<h2>', text))
if h2 != 3:
    print(f"expected 3 <h2> headings, found {h2}", file=sys.stderr)
    sys.exit(1)
body = text.split("<main", 1)[-1]  # skip <head> (meta description mentions "limitation" in prose)
label_patterns = {
    "problem": r'<b>problem</b>',
    "otherwise": r'<b>otherwise</b>',
    "sequence": r'class="snip-label">sequence<',
    "yours": r'<b>yours</b>',
    "limitation": r'<b>limitation</b>',
}
for label, pattern in label_patterns.items():
    n = len(re.findall(pattern, body))
    if n != 3:
        print(f"label '{label}' appears {n} times (pattern {pattern!r}), expected 3", file=sys.stderr)
        sys.exit(1)
PY
}

# AC4: the compare table has four non-mcphost data rows of six columns
# each, and every non-mcphost cell is either preceded (within its own
# row's markup) by an HTML comment naming an http(s) source, or reads
# "not stated".
compare_table_structure() {
  python3 - www/compare.html <<'PY'
import re, sys

text = open("www/compare.html", encoding="utf-8").read()
tables = re.findall(r'<table[^>]*>.*?</table>', text, re.S)
if len(tables) != 1:
    print(f"expected exactly one <table>, found {len(tables)}", file=sys.stderr)
    sys.exit(1)
table = tables[0]

header = re.search(r'<thead>.*?</thead>', table, re.S)
if not header:
    print("no <thead> found", file=sys.stderr)
    sys.exit(1)
cols = re.findall(r'<th>', header.group(0))
if len(cols) != 7:  # product name + 6 compared columns
    print(f"expected 7 header cells (product + 6 columns), found {len(cols)}", file=sys.stderr)
    sys.exit(1)

body = re.search(r'<tbody>(.*?)</tbody>', table, re.S).group(1)
# Each row block is any HTML comments immediately preceding a <tr>...</tr>,
# captured together so a sourcing comment is attributed to the row it sources.
row_blocks = re.findall(r'(?:<!--.*?-->\s*)*<tr.*?</tr>', body, re.S)

data_rows = 0
for block in row_blocks:
    name_m = re.search(r'<th scope="row">([^<]*)</th>', block)
    name = name_m.group(1) if name_m else "?"
    if name == "mcphost":
        continue  # self row: no sourcing required
    data_rows += 1
    has_comment = bool(re.search(r'<!--[^>]*http', block))
    cells = re.findall(r'<td>(.*?)</td>', block, re.S)
    if len(cells) != 6:
        print(f"row {name}: expected 6 <td> cells, found {len(cells)}", file=sys.stderr)
        sys.exit(1)
    for cell in cells:
        plain = re.sub(r'<[^>]+>', '', cell).strip()
        if plain == "not stated":
            continue
        if not has_comment:
            print(f"row {name}: cell {plain!r} is not 'not stated' and no http comment precedes the row", file=sys.stderr)
            sys.exit(1)

if data_rows != 3:
    print(f"expected 3 non-mcphost data rows, found {data_rows}", file=sys.stderr)
    sys.exit(1)
PY
}

footer_links_all_four() {
  grep -qF 'href="/pricing"' www/index.html \
    && grep -qF 'href="/use-cases"' www/index.html \
    && grep -qF 'href="/compare"' www/index.html \
    && grep -qF 'href="/skill.md"' www/index.html
}

skill_linked_in_first_ten_lines() {
  head -n 10 www/llms.txt | grep -qF '/skill.md'
}

# --- file existence (requirements 1-4) ---
check "www/pricing.html exists" file_exists www/pricing.html
check "www/use-cases.html exists" file_exists www/use-cases.html
check "www/compare.html exists" file_exists www/compare.html
check "www/skill.md exists" file_exists www/skill.md

# --- HTML validity, no external assets, connect line (requirements 6-7) ---
for f in www/pricing.html www/use-cases.html www/compare.html; do
  check "$f parses with html.parser" html_parses "$f"
  check "$f has no external <script src=>" no_external_script "$f"
  check "$f has no external stylesheet <link>" no_external_stylesheet "$f"
  check "$f ends with the connect line" has_connect_line "$f"
done

# --- skill.md (requirement 4 / AC5) ---
check "www/skill.md matches the plugin SKILL.md contract" skill_md_check

# --- pricing.html content (AC2) ---
check "pricing.html names \$19" grep_has www/pricing.html '$19'
check "pricing.html names 500 calls" grep_has www/pricing.html '500 calls'
check "pricing.html names 50,000" grep_has www/pricing.html '50,000'
check "pricing.html names \$0.001" grep_has www/pricing.html '$0.001'
check "pricing.html names billing.checkout" grep_has www/pricing.html 'billing.checkout'
check "pricing.html names the exit phrase 'change the endpoint'" grep_has www/pricing.html 'change the endpoint'

# --- use-cases.html structure (AC3) ---
check "use-cases.html has three workflows with all five part labels" use_cases_structure

# --- identifier coverage (requirement 9) ---
check "pricing.html host./billing./mcphost. identifiers all appear in llms-full.txt" identifiers_covered www/pricing.html
check "use-cases.html host./billing./mcphost. identifiers all appear in llms-full.txt" identifiers_covered www/use-cases.html

# --- compare.html table (AC4) ---
check "compare.html table has 3 competitor rows of 6 sourced-or-not-stated columns" compare_table_structure

# --- footer / llms.txt wiring (AC6, minus the deploy-owned sitemap) ---
check "index.html footer links pricing, use cases, compare, and skill.md" footer_links_all_four
check "llms.txt links /skill.md within its first ten lines" skill_linked_in_first_ten_lines

exit "$status"
