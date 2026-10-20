#!/usr/bin/env bash
# Run the scope guard from a pre-commit hook.
#
# verify-scope.sh --staged judges the index, which is what the commit is about to contain. Plain
# verify-scope.sh diffs base..HEAD, and at pre-commit time HEAD does not have the files yet.
#
# The guard diffs against the upstream base recorded in ictrek.app/UPSTREAM.yml. That commit is
# only present after `git fetch upstream --tags`, so on a fresh clone the guard would refuse every
# commit for a reason the author cannot act on. Skipping (with a note) keeps the hook usable; CI
# still runs the guard, and there the base is always fetched.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(git -C "${HERE}" rev-parse --show-toplevel)"
UPSTREAM_FILE="${REPO_ROOT}/ictrek.app/UPSTREAM.yml"

base=""
if [[ -f "${UPSTREAM_FILE}" ]]; then
  base="$(python3 - "${UPSTREAM_FILE}" <<'PY'
import sys
sha = ""
for line in open(sys.argv[1], encoding="utf-8"):
    if line.startswith("base_sha:"):
        sha = line.split(":", 1)[1].strip().strip('"')
print(sha)
PY
)"
fi

if [[ -z "${base}" ]]; then
  echo "[scope] no base_sha in ictrek.app/UPSTREAM.yml; skipping"
  exit 0
fi

if ! git -C "${REPO_ROOT}" rev-parse --verify --quiet "${base}^{commit}" >/dev/null; then
  echo "[scope] upstream base ${base} is not a local ref; skipping"
  echo "[scope] run 'git fetch upstream --tags' to enable this check"
  exit 0
fi

exec bash "${HERE}/verify-scope.sh" --staged
