#!/usr/bin/env bash
# recipe-task-sync-check.sh -- PRD-mcphost-recipe-golden-path-tests R6.
#
# Compares every `recipe:`-tagged task of a synthorg consumer-task corpus
# (corpora/mcphost/consumer-tasks.yaml) with the yaml this repo generates for
# that recipe (examples/<recipe>/synthorg-task.yaml, written by
# `cargo xtask recipes` from recipes/<recipe>.toml): its `task` text, its
# `gold` block and its `wall_budget_s`. Prints one DRIFT line per differing
# field naming the task id and both values; exit 1 if any task drifted, else 0.
#
# A corpus tag matches a recipe when, after dropping a leading `mcphost-` and
# a trailing `-recipe`, it equals the recipe name or starts with `<name>-`
# (`mcphost-share-a-tool-not-a-key` -> share-a-tool). A tagged task whose
# recipe has no generated yaml is reported as skipped, not as drift.
#
# Usage: recipe-task-sync-check.sh <consumer-tasks.yaml> [repo-root]
set -uo pipefail

corpus="${1:-}"
root="${2:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
if [[ -z "$corpus" || ! -f "$corpus" ]]; then
  echo "usage: recipe-task-sync-check.sh <consumer-tasks.yaml> [repo-root]" >&2
  exit 2
fi

exec python3 -I - "$corpus" "$root" <<'PY'
import glob
import os
import re
import sys

corpus_path, root = sys.argv[1], sys.argv[2]
BLOCK = {">", ">-", ">+", "|", "|-", "|+"}


def indent_of(line):
    return len(line) - len(line.lstrip(" "))


def parse_block(lines, i, indent):
    """Read `key: value` lines at exactly `indent`; returns (dict, next_i)."""
    out = {}
    n = len(lines)
    while i < n:
        line = lines[i]
        if not line.strip() or line.lstrip().startswith("#"):
            i += 1
            continue
        ind = indent_of(line)
        if ind < indent:
            break
        if ind > indent or ":" not in line:
            i += 1
            continue
        key, _, val = line.strip().partition(":")
        val = val.strip()
        i += 1
        if val in BLOCK:
            parts = []
            while i < n and (not lines[i].strip() or indent_of(lines[i]) > indent):
                if lines[i].strip():
                    parts.append(lines[i].strip())
                i += 1
            out[key] = " ".join(parts)
        elif val == "":
            j = i
            while j < n and (not lines[j].strip() or lines[j].lstrip().startswith("#")):
                j += 1
            if j < n and indent_of(lines[j]) > indent:
                out[key], i = parse_block(lines, j, indent_of(lines[j]))
            else:
                out[key] = ""
        else:
            if len(val) >= 2 and val[0] == val[-1] and val[0] in "\"'":
                val = val[1:-1]
            out[key] = val
    return out, i


def parse_tasks(text):
    tasks, chunk = [], None
    for line in text.split("\n"):
        if re.match(r"^  - ", line):
            if chunk is not None:
                tasks.append(chunk)
            chunk = ["    " + line[4:]]
        elif chunk is not None:
            if line.strip() and indent_of(line) < 4 and not line.lstrip().startswith("#"):
                tasks.append(chunk)
                chunk = None
            else:
                chunk.append(line)
    if chunk is not None:
        tasks.append(chunk)
    return [parse_block(c, 0, 4)[0] for c in tasks]


def norm(s):
    return re.sub(r"\s+", " ", str(s)).strip()


def tag_matches(tag, name):
    t = tag
    if t.startswith("mcphost-"):
        t = t[len("mcphost-"):]
    if t.endswith("-recipe"):
        t = t[: -len("-recipe")]
    return t == name or t.startswith(name + "-")


generated = {}
for path in sorted(glob.glob(os.path.join(root, "examples", "*", "synthorg-task.yaml"))):
    with open(path, encoding="utf-8") as fh:
        for task in parse_tasks(fh.read()):
            if task.get("recipe"):
                generated[task["recipe"]] = task

with open(corpus_path, encoding="utf-8") as fh:
    corpus = parse_tasks(fh.read())

checked = drifted = 0
for task in corpus:
    tag = task.get("recipe")
    if not tag:
        continue
    names = [n for n in generated if tag_matches(tag, n)]
    if not names:
        print(f"recipe-task-sync-check: skipped {task.get('id', '?')} [{tag}]: no generated yaml for this recipe")
        continue
    want = generated[names[0]]
    checked += 1
    tid = task.get("id", "?")
    diffs = []
    for field in ("task", "wall_budget_s"):
        if norm(task.get(field, "")) != norm(want.get(field, "")):
            diffs.append((field, norm(task.get(field, "<missing>")), norm(want.get(field, "<missing>"))))
    cg, wg = task.get("gold") or {}, want.get("gold") or {}
    for key in sorted(set(cg) | set(wg)):
        if norm(cg.get(key, "<missing>")) != norm(wg.get(key, "<missing>")):
            diffs.append((f"gold.{key}", norm(cg.get(key, "<missing>")), norm(wg.get(key, "<missing>"))))
    for field, have, generated_value in diffs:
        print(f"recipe-task-sync-check: DRIFT {tid} [{names[0]}] {field}: corpus={have} generated={generated_value}")
    drifted += 1 if diffs else 0

print(f"recipe-task-sync-check: {checked} task(s) checked, {drifted} drifted")
sys.exit(1 if drifted else 0)
PY
