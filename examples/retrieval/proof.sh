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
#   examples/retrieval/proof.sh [URL]
#       no flag: runs --create-check against URL ($1, or $MCPHOST_URL), the
#       way `mcphost examples prove` spawns it (`bash proof.sh <url>`).
#
# Exit code is the chosen proof's exit code (2 on a bad flag).

set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"

if [[ -n "${MCPHOST_PROVE_DRY:-}" ]]; then
  case "${1:-}" in
    ""|http://*|https://*|--create-check|--refresh-check)
      echo "plan: retrieval proof ${1:---create-check} against ${MCPHOST_URL:-${1:-default}}"
      exit 0 ;;
  esac
fi

case "${1:-}" in
  --create-check) exec bash "$here/proof-create.sh" "$@" ;;
  --refresh-check) exec bash "$here/proof-refresh.sh" "$@" ;;
  ""|http://*|https://*) exec bash "$here/proof-create.sh" --create-check "$@" ;;
  *)
    echo "usage: proof.sh --create-check [URL] | --refresh-check" >&2
    exit 2
    ;;
esac
