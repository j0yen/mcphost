#!/usr/bin/env bash
# spec-fields-doc-check.sh — PRD-mcphost-spec-unknown-field-rejection
# requirement 6 (AC7). Diffs each registered kind's own
# `Kind::known_spec_fields()` list against its `docs/kinds/<kind>.md`
# worked example spec, via the `mcphost spec-fields-check` subcommand
# (`mcphost::kinds::docs::check_docs_against_known_fields`) -- the single
# source both `host.tool_publish`'s `unknown_spec_field` check and this
# script read from, so docs and parser cannot drift without this failing.
#
# Prints one `kind=<k> fields=<n> doc_in_sync=<bool>` line per kind (stdout)
# and, on a mismatch, the offending field name(s) (stderr), then exits 1.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo run --quiet --bin mcphost -- spec-fields-check
