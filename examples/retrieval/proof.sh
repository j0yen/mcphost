#!/usr/bin/env bash
# proof.sh -- runnable proofs for the retrieval recipe; dispatches on the flag.
#
#   examples/retrieval/proof.sh --create-check [URL]
#       one host.index.create call turns two documents into a named index
#       (PRD-mcphost-index-create-from-sources-b AC 4); see proof-create.sh.
#   examples/retrieval/proof.sh --refresh-check
#       the index follows a re-put document, behind_by 1 then 0
#       (PRD-mcphost-index-refresh-on-change AC 8); see proof-refresh.sh.
#
# Exit code is the chosen proof's exit code (2 on a bad flag).

set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"

case "${1:-}" in
  --create-check) exec bash "$here/proof-create.sh" "$@" ;;
  --refresh-check) exec bash "$here/proof-refresh.sh" "$@" ;;
  *)
    echo "usage: proof.sh --create-check [URL] | --refresh-check" >&2
    exit 2
    ;;
esac
