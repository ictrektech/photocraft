#!/usr/bin/env bash
# Fail when a workflow other than the fork's own two is present.
#
# Upstream's workflows ride in with every sync rebase. The prune commit at the head of the VOS
# series deletes them, so one showing up here means the rebase brought it back. A missing or empty
# .github/workflows is a separate problem, but it must not read as "nothing foreign found": with no
# workflows at all CI never runs, and the fork would push blind.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(git -C "${HERE}" rev-parse --show-toplevel)"
WORKFLOWS="${REPO_ROOT}/.github/workflows"

keep='vos-release\.yml|vos-ci\.yml'

if [[ ! -d "${WORKFLOWS}" ]]; then
  echo "[workflows] ${WORKFLOWS} is missing; the fork's CI cannot run" >&2
  exit 1
fi

shopt -s nullglob
present=("${WORKFLOWS}"/*.yml "${WORKFLOWS}"/*.yaml)
shopt -u nullglob

if (( ${#present[@]} == 0 )); then
  echo "[workflows] ${WORKFLOWS} has no workflow files; the fork's CI cannot run" >&2
  exit 1
fi

foreign=()
for file in "${present[@]}"; do
  base="$(basename "${file}")"
  [[ "${base}" =~ ^(${keep})$ ]] && continue
  foreign+=("${base}")
done

if (( ${#foreign[@]} > 0 )); then
  echo "[workflows] these workflows are not the fork's own:" >&2
  printf '  .github/workflows/%s\n' "${foreign[@]}" >&2
  echo >&2
  echo "Upstream workflows ride in with a sync rebase; the prune commit deletes them." >&2
  echo "Delete the file(s) above, or add them to .github/workflows deliberately." >&2
  exit 1
fi

echo "[workflows] only the fork's own workflows are present"
