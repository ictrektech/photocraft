#!/usr/bin/env bash
# Run before a push: a release tag must agree with ictrek.app/VERSION, because the release
# workflow refuses one whose tree disagrees. Only a push that carries the tag can be wrong, so
# everything else passes.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(git -C "${HERE}" rev-parse --show-toplevel)"
VERSION_FILE="${REPO_ROOT}/ictrek.app/VERSION"

[[ -f "${VERSION_FILE}" ]] || exit 0
version="$(tr -d '[:space:]' < "${VERSION_FILE}")"
[[ -n "${version}" ]] || exit 0

tag="vos-photocraft-v${version}"
if git -C "${REPO_ROOT}" rev-parse --verify --quiet "refs/tags/${tag}^{commit}" >/dev/null; then
  # The tag exists; its tree must be the one VERSION describes. A tag whose tree has no VERSION at
  # all is a different problem from one that disagrees, and saying which saves a confusing hunt.
  if ! tagged_version="$(git -C "${REPO_ROOT}" show "refs/tags/${tag}:ictrek.app/VERSION" 2>/dev/null)"; then
    echo "[version] tag ${tag} has no ictrek.app/VERSION; cannot check it against ${version}" >&2
    exit 1
  fi
  tagged_version="$(tr -d '[:space:]' <<< "${tagged_version}")"
  if [[ "${tagged_version}" != "${version}" ]]; then
    echo "[version] tag ${tag} carries VERSION '${tagged_version}', the working tree says '${version}'" >&2
    exit 1
  fi
  echo "[version] ${tag} agrees with ictrek.app/VERSION"
else
  echo "[version] no tag ${tag} yet; nothing to compare"
fi
