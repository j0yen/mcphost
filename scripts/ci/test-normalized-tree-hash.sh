#!/usr/bin/env bash
# Proof of scripts/ci/normalized-tree-hash.sh in a scratch git repo. Run from
# anywhere; needs only git, awk, sed. The CI gate job runs this first.
set -euo pipefail

script=$(cd "$(dirname "$0")" && pwd)/normalized-tree-hash.sh
scratch_root=${NTREE_TEST_ROOT:-${RUNNER_TEMP:-/tmp}}
mkdir -p "$scratch_root"
work=$(mktemp -d "$scratch_root/ntree-test.XXXXXX")
trap 'rm -rf "$work"' EXIT

pass=0
fail=0
ok() { pass=$((pass + 1)); echo "PASS  $1"; }
bad() { fail=$((fail + 1)); echo "FAIL  $1"; }
expect_eq() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 ($2 != $3)"; fi; }
expect_ne() { if [ "$2" != "$3" ]; then ok "$1"; else bad "$1 (both $2)"; fi; }

cd "$work"
git init -q -b main .
git config user.email t@example.invalid
git config user.name t
git config commit.gpgsign false

write_files() { # <version>   (CHANGELOG.md is handled by write_changelog / release_top)
  mkdir -p src plugin/.claude-plugin
  cat > Cargo.toml <<EOF
[workspace]
members = []

[package]
name = "demo"
version = "$1"
edition = "2024"

[dependencies]
serde = "1"
foo = { version = "1" }

[dependencies.bar]
version = "1"
features = ["x"]
EOF
  cat > Cargo.lock <<EOF
version = 4

[[package]]
name = "demo"
version = "$1"
dependencies = ["serde"]

[[package]]
name = "serde"
version = "1.0.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "aaaa"
EOF
  printf '{\n  "name": "demo",\n  "version": "%s"\n}\n' "$1" > plugin/.claude-plugin/plugin.json
  printf '{\n  "name": "demo",\n  "version": "%s"\n}\n' "$1" > plugin/.mcp.json
}

write_changelog() {
  cat > CHANGELOG.md <<CLEOF
# Changelog

## v1.0.0 — 2026-10-01

- base entry

## v0.9.0 — 2026-09-01

- old entry
CLEOF
}

# What the daemon's version commit does: a new top release section.
release_top() { # <version>
  { printf '# Changelog\n\n## v%s — 2026-10-02\n\nReleased.\n\n' "$1"; sed '1,2d' CHANGELOG.md; } > CHANGELOG.md.new
  mv CHANGELOG.md.new CHANGELOG.md
}

commit() { git add -A && git commit -q -m "$1"; }

# base: what origin/<base> holds
write_files 1.0.0
write_changelog
echo 'fn main() {}' > src/main.rs
echo 'fn helper() {}' > src/helper.rs
echo 'fn main() {}' > build.rs
printf '[toolchain]\nchannel = "1.95"\n' > rust-toolchain.toml
mkdir -p .github/workflows scripts/ci
printf 'name: ci\n' > .github/workflows/ci.yml
printf '#!/usr/bin/env bash\necho a\n' > scripts/ci/some-check.sh
commit base
git update-ref refs/remotes/origin/main HEAD

# code commit on a branch off base
git checkout -q -b pr
echo 'fn main() { println!("x"); }' > src/main.rs
commit code
code_tree=$(bash "$script" origin/main HEAD)

# version-only commit on top of the passed code tree
write_files 1.0.1
release_top 1.0.1
commit release
ver_tree=$(bash "$script" origin/main HEAD)
expect_eq "version-only commit on top of a code commit has the same NTREE" "$code_tree" "$ver_tree"

# the raw trees differ (so the normalization is what makes them equal)
expect_ne "raw HEAD trees differ across the version bump" "$(git rev-parse "HEAD~1^{tree}")" "$(git rev-parse "HEAD^{tree}")"

# the working tree and real index were never touched
expect_eq "working tree is clean after hashing" "$(git status --porcelain)" ""

# a code change on top differs
echo 'fn main() { println!("y"); }' > src/main.rs
commit code2
expect_ne "a further code change gives a different NTREE" "$ver_tree" "$(bash "$script" origin/main HEAD)"
git reset -q --hard HEAD~1

# (b) a second stacked release section + Cargo bump is still version-only
write_files 1.0.2
release_top 1.0.2
commit release2
expect_eq "top-section-only changelog edit + Cargo bump keeps the NTREE" "$ver_tree" "$(bash "$script" origin/main HEAD)"
git reset -q --hard HEAD~1

# (b2) unreleased prose above the headings (before the daemon adds one) is new top material too
{ printf '# Changelog\n\nA new feature note.\n\n'; sed '1,2d' CHANGELOG.md; } > CHANGELOG.md.new && mv CHANGELOG.md.new CHANGELOG.md
commit top-prose
expect_eq "new prose above the first heading keeps the NTREE" "$ver_tree" "$(bash "$script" origin/main HEAD)"
git reset -q --hard HEAD~1

# (a) an edit to an OLDER changelog entry changes the NTREE
sed -i 's/^- old entry$/- edited old entry/' CHANGELOG.md
commit old-entry-edit
expect_ne "an edit to an older changelog entry changes the NTREE" "$ver_tree" "$(bash "$script" origin/main HEAD)"
git reset -q --hard HEAD~1

# (a2) the base's newest entry edited, and (a3) an older section removed
sed -i 's/^- base entry$/- edited base entry/' CHANGELOG.md
commit base-entry-edit
expect_ne "an edit to the base's newest entry changes the NTREE" "$ver_tree" "$(bash "$script" origin/main HEAD)"
git reset -q --hard HEAD~1
head -n -4 CHANGELOG.md > CHANGELOG.md.new && mv CHANGELOG.md.new CHANGELOG.md
commit old-section-removed
expect_ne "removing an older changelog section changes the NTREE" "$ver_tree" "$(bash "$script" origin/main HEAD)"
git reset -q --hard HEAD~1

# (d) the Cargo.lock file header (`version = 4`, the lockfile format) is not blanked
sed -i '1s/^version = 4$/version = 3/' Cargo.lock
commit lock-header
expect_ne "a Cargo.lock header change is not normalized away" "$ver_tree" "$(bash "$script" origin/main HEAD)"
git reset -q --hard HEAD~1

# a dependency change in Cargo.toml (not the version line) differs
sed -i 's/^serde = "1"/serde = "2"/' Cargo.toml
commit dep-toml
expect_ne "a Cargo.toml dependency change is not normalized away" "$ver_tree" "$(bash "$script" origin/main HEAD)"
git reset -q --hard HEAD~1

# dependency versions in every Cargo.toml dependency form differ (the version
# blanking is limited to [package] / [workspace.package])
sed -i 's/^foo = { version = "1" }/foo = { version = "2" }/' Cargo.toml
commit dep-inline
expect_ne "an inline { version = } dependency bump is not normalized away" "$ver_tree" "$(bash "$script" origin/main HEAD)"
git reset -q --hard HEAD~1

sed -i '/^\[dependencies.bar\]/,/^$/s/^version = "1"/version = "2"/' Cargo.toml
commit dep-table
expect_ne "a [dependencies.x] table version bump is not normalized away" "$ver_tree" "$(bash "$script" origin/main HEAD)"
git reset -q --hard HEAD~1

# build / toolchain / CI inputs are part of the key
for f in build.rs rust-toolchain.toml .github/workflows/ci.yml scripts/ci/some-check.sh; do
  echo '# changed' >> "$f"
  commit "touch-$f"
  expect_ne "a change to $f changes the NTREE" "$ver_tree" "$(bash "$script" origin/main HEAD)"
  git reset -q --hard HEAD~1
done

# a deleted file and a renamed src file differ
git rm -q src/helper.rs
commit delete-file
expect_ne "a deleted file changes the NTREE" "$ver_tree" "$(bash "$script" origin/main HEAD)"
git reset -q --hard HEAD~1
git mv src/helper.rs src/helper2.rs
commit rename-file
expect_ne "a renamed src file changes the NTREE" "$ver_tree" "$(bash "$script" origin/main HEAD)"
git reset -q --hard HEAD~1

# a mode change differs: on a non-version file, and on a version file riding a
# version bump (the swap must not take the base's mode)
chmod +x src/main.rs
commit chmod-code
expect_ne "chmod +x on a non-version file changes the NTREE" "$ver_tree" "$(bash "$script" origin/main HEAD)"
git reset -q --hard HEAD~1
chmod +x Cargo.toml
commit chmod-cargo-toml
expect_ne "chmod +x on Cargo.toml alone changes the NTREE" "$ver_tree" "$(bash "$script" origin/main HEAD)"
git reset -q --hard HEAD~1
git checkout -q -b modebump HEAD~1   # the code commit, before the release bump
write_files 1.0.1
release_top 1.0.1
chmod +x Cargo.toml
commit release-with-chmod
expect_ne "chmod +x on a version file riding a version bump changes the NTREE" "$ver_tree" "$(bash "$script" origin/main HEAD)"
git checkout -q pr
git branch -q -D modebump

# a registry package version change in Cargo.lock differs
awk '/name = "serde"/ { s = 1 } s && /^version = / { print "version = \"1.0.9\""; s = 0; next } { print }' Cargo.lock > Cargo.lock.new && mv Cargo.lock.new Cargo.lock
commit dep-lock
expect_ne "a Cargo.lock registry-version change is not normalized away" "$ver_tree" "$(bash "$script" origin/main HEAD)"
git reset -q --hard HEAD~1

# a non-version change in plugin.json differs
sed -i 's/"name": "demo"/"name": "other"/' plugin/.claude-plugin/plugin.json
commit plugin-name
expect_ne "a plugin.json non-version change is not normalized away" "$ver_tree" "$(bash "$script" origin/main HEAD)"
git reset -q --hard HEAD~1

# rebase onto a moved main: different base code -> different NTREE
git checkout -q main
echo '// moved' > src/other.rs
commit main-moved
git update-ref refs/remotes/origin/main HEAD
git checkout -q pr
git rebase -q main
rebased=$(bash "$script" origin/main HEAD)
expect_ne "rebase onto moved main changes NTREE" "$ver_tree" "$rebased"

# missing version files must not crash: drop plugin files and CHANGELOG on HEAD
git rm -q -r plugin CHANGELOG.md
commit drop-version-files
if out=$(bash "$script" origin/main HEAD 2>&1) && [ -n "$out" ]; then ok "missing version files on HEAD do not crash"; else bad "missing version files on HEAD crashed: $out"; fi

# ... and absent on the base side only
git checkout -q -b nobase main
git rm -q -r plugin CHANGELOG.md
commit base-without
git update-ref refs/remotes/origin/nobase HEAD
git checkout -q pr
if out=$(bash "$script" origin/nobase HEAD 2>&1) && [ -n "$out" ]; then ok "version files missing on the base do not crash"; else bad "version files missing on base crashed: $out"; fi

# unresolvable base ref falls back to the plain tree hash
out=$(bash "$script" origin/does-not-exist HEAD 2>/dev/null)
expect_eq "unknown base ref falls back to the plain tree hash" "$out" "$(git rev-parse 'HEAD^{tree}')"

echo "normalized-tree-hash: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
