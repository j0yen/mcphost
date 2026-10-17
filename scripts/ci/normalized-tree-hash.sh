#!/usr/bin/env bash
# Print NTREE: the tree hash of <rev> (default HEAD) with the VERSION FILES
# replaced by their content on <base-ref> (e.g. origin/main).
#
# Used by the `static analysis + core suites` job as the key of a sound cache:
# the heavy suite is skipped only when the CODE tree is byte-identical to one
# that already passed, so a version-only commit on top of a passed tree has the
# same NTREE and costs seconds, while a rebase onto moved main (different code)
# has a different NTREE and runs everything.
#
# Usage: scripts/ci/normalized-tree-hash.sh <base-ref> [rev]
#
# The computation uses a TEMPORARY index; the working tree and the real index
# are never touched. A version file absent on either side stays as it is. If
# <base-ref> does not resolve, nothing is normalized (the key is then the plain
# tree hash: still sound, just never shared across a version bump).
#
# Soundness beyond a plain path swap: Cargo.toml, Cargo.lock and the two JSON
# manifests also carry code-affecting content (dependencies, features). They are
# swapped for the base blob ONLY when HEAD and base are equal once the version
# line itself is blanked; otherwise the HEAD blob stays and the key differs.
#
# CHANGELOG.md is read by contract tests (heading parse, host.changelog via
# include_str!), so it is NOT swapped wholesale. Only the NEW TOP material a
# release adds is normalized: if HEAD's file ends with base's file from its
# first `## v` heading onward (every older entry byte-identical), HEAD's
# changelog is replaced by base's; if any older entry was edited, reordered or
# removed, the HEAD blob stays and the key differs. The shape of the new top
# section (heading format, order, versions) is proven separately, on every
# run, by scripts/ci/version-files-consistency.sh.
set -euo pipefail

base=${1:?usage: normalized-tree-hash.sh <base-ref> [rev]}
rev=${2:-HEAD}

# The files a release bump rewrites. plugin.json and .mcp.json live under
# plugin/ in this repo; the bare names are kept for repos that have them at root.
VERSION_FILES=(
  Cargo.toml
  Cargo.lock
  plugin.json
  .mcp.json
  plugin/.claude-plugin/plugin.json
  plugin/.mcp.json
  CHANGELOG.md
)

# Blank the version line(s) that a release bump rewrites, so two blobs that
# differ only there compare equal.
blank_version() { # <path>; blob on stdin
  case "$1" in
    Cargo.toml)
      awk '/^\[/ { sec = $0 }
           (sec == "[package]" || sec == "[workspace.package]") && /^version[ \t]*=/ { print "version = BLANK"; next }
           { print }' ;;
    Cargo.lock)
      # Only `version =` lines inside a [[package]] block with no `source =`
      # line (a path package, i.e. ours) are blanked. The file header's own
      # `version = 4` (lockfile format) and any registry/git package version
      # are dependency/format changes and must stay visible.
      awk 'function flush() {
             for (i = 1; i <= n; i++) {
               l = blk[i]
               if (in_pkg && !has_source && l ~ /^version = /) l = "version = BLANK"
               print l
             }
             n = 0; has_source = 0
           }
           /^\[\[package\]\]/ { flush(); in_pkg = 1 }
           { blk[++n] = $0; if ($0 ~ /^source = /) has_source = 1 }
           END { flush() }' ;;
    *.json)
      sed -E 's/^([[:space:]]*"version"[[:space:]]*:[[:space:]]*)"[^"]*"/\1"BLANK"/' ;;
    *) cat ;;
  esac
}

# CHANGELOG.md: print the base blob if HEAD's changelog is base's changelog
# (from its first `## v` heading on) with only NEW material above it.
changelog_is_base_plus_new_top() { # <head-blob> <base-blob>
  local bf hf bsz
  bf=$(mktemp) hf=$(mktemp)
  git cat-file blob "$2" | awk '/^## v/ { on = 1 } on { print }' > "$bf"
  bsz=$(wc -c < "$bf")
  if [ "$bsz" -eq 0 ]; then rm -f "$bf" "$hf"; return 1; fi
  git cat-file blob "$1" > "$hf"
  if [ "$(wc -c < "$hf")" -ge "$bsz" ] && tail -c "$bsz" "$hf" | cmp -s - "$bf"; then
    rm -f "$bf" "$hf"; return 0
  fi
  rm -f "$bf" "$hf"; return 1
}

blob() { git rev-parse -q --verify "$1:$2" 2>/dev/null || true; }
mode() { git ls-tree "$1" -- "$2" | awk '{ print $1 }'; }

if ! git rev-parse -q --verify "$base^{commit}" >/dev/null 2>&1; then
  echo "normalized-tree-hash: base ref $base not found; hashing $rev unnormalized" >&2
  git rev-parse "$rev^{tree}"
  exit 0
fi

idx=$(mktemp)
trap 'rm -f "$idx"' EXIT
export GIT_INDEX_FILE=$idx
git read-tree "$rev"

for f in "${VERSION_FILES[@]}"; do
  hb=$(blob "$rev" "$f")
  bb=$(blob "$base" "$f")
  [ -n "$hb" ] && [ -n "$bb" ] || continue   # absent on either side: leave as is
  [ "$hb" != "$bb" ] || continue
  if [ "$f" = CHANGELOG.md ]; then
    changelog_is_base_plus_new_top "$hb" "$bb" || continue   # an older entry moved: keep HEAD blob
  else
    h=$(git cat-file blob "$hb" | blank_version "$f" | git hash-object --stdin)
    b=$(git cat-file blob "$bb" | blank_version "$f" | git hash-object --stdin)
    [ "$h" = "$b" ] || continue              # more than the version moved: keep HEAD blob
  fi
  git update-index --cacheinfo "$(mode "$base" "$f"),$bb,$f"
done

git write-tree
