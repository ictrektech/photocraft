#!/usr/bin/env bash
# Guard against the fork drifting away from upstream: list (or check) every file that differs from
# the upstream base commit, and fail when one of them is outside the VOS whitelist.
#
#   ./scripts/verify-scope.sh            # check, non-zero when a file is out of scope
#   ./scripts/verify-scope.sh --list     # just print what differs, always exit 0
#   ./scripts/verify-scope.sh --base v0.8.2
#   ./scripts/verify-scope.sh --staged   # check the index instead of HEAD (pre-commit hook)
#
# Why it matters: mirror tracks upstream untouched and main carries the VOS packaging, so
# `git rebase` stays a five-minute job. A change to a core file that this script rejects is still
# allowed -- but it must be added to the whitelist deliberately, in the same commit that makes it.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_DIR="$(cd "${HERE}/.." && pwd)"
REPO_ROOT="$(git -C "${APP_DIR}" rev-parse --show-toplevel)"
UPSTREAM_FILE="${APP_DIR}/UPSTREAM.yml"

# Where a VOS change may live. Everything else must come from upstream unchanged.
WHITELIST=(
  "README.md"
  "ictrek.app/"
  "docker/"
  "scripts/vos-dev.sh"
  "scripts/vos-fonts.sh"
  "Makefile"
  ".github/workflows/"
  "docs/vos-integration.md"
  "apps/photocraft-web/Trunk.vos-dev.toml"
  "ATTRIBUTION.md"
  ".pre-commit-config.yaml"
  ".gitignore"
  ".dockerignore"
  "packaging/web/nginx.conf"
)

# Upstream code paths where the fork deliberately diverges. Touching these is allowed — the fork
# does change real code — but every changed file under them shows up in `verify-scope.sh --list`,
# which is the conflict checklist for the next upstream rebase. Keep this list SHORT: each entry
# here is a file that may need hand-merging on every sync.
DEVIATION=(
  "crates/"
  "apps/"
  "xtask/"
  "Cargo.toml"
  "Cargo.lock"
)

die() { echo "[scope][ERROR] $*" >&2; exit 1; }
log() { echo "[scope] $*"; }

base=""
list_only=0
staged=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --base) base="${2:-}"; shift 2 ;;
    --list) list_only=1; shift ;;
    --staged) staged=1; shift ;;
    -h|--help)
      # The usage block is the run of "#   ./scripts/..." lines in the header; print that instead of
      # a fixed line range, which silently drops a new flag when one is added.
      grep -E '^#   \./scripts/' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
      exit 0 ;;
    *) die "unknown argument: $1" ;;
  esac
done

cd "$REPO_ROOT"

if [[ -z "$base" ]]; then
  if [[ -f "$UPSTREAM_FILE" ]]; then
    # Prefer base_sha: the exact upstream commit the fork is based on. It is the upstream release
    # tag's commit under the current scheme (main pinned to a tag), but a tagged main that later
    # drifted would leave the tag behind the fork point, and comparing against v<core> would then
    # report every upstream commit since the tag as fork drift. Fall back to the tag when absent.
    base="$(python3 - "$UPSTREAM_FILE" <<'PY'
import sys
core = sha = ""
for line in open(sys.argv[1], encoding="utf-8"):
    if line.startswith("core:"):
        core = line.split(":", 1)[1].strip().strip('"')
    elif line.startswith("base_sha:"):
        sha = line.split(":", 1)[1].strip().strip('"')
print(sha or ("v" + core))
PY
)"
  fi
fi
[[ -n "$base" ]] || die "no --base given and ${UPSTREAM_FILE} has no core: line"

if ! git rev-parse --verify --quiet "${base}^{commit}" >/dev/null; then
  die "the upstream base ${base} is not a local ref: git fetch upstream --tags (or pass --base <sha>)"
fi
if ! git merge-base --is-ancestor "${base}" HEAD; then
  die "HEAD is not a descendant of ${base}: fetch upstream and rebase main onto the new upstream release first
  (see plan/vos-integration.md, 'Upstream sync')"
fi
log "base ${base} ($(git rev-parse --short "$base"))"

if (( staged )); then
  # A pre-commit hook runs before the commit exists, so HEAD does not contain the files it has to
  # judge. The index does: that is what the commit is about to hold. Files that are neither staged
  # nor committed are not judged here, because the commit will not contain them either.
  changed="$({ git diff --cached --name-only --diff-filter=ACMRD; git diff --name-only "${base}" HEAD; } | sort -u)"
else
  changed="$(git diff --name-only "${base}" HEAD)"
fi
if [[ -z "$changed" ]]; then
  log "no changes against the base"
  exit 0
fi

allowed() {
  local path="$1" entry
  for entry in "${WHITELIST[@]}"; do
    case "$path" in
      "$entry"*) return 0 ;;
    esac
  done
  return 1
}

deviation() {
  local path="$1" entry
  for entry in "${DEVIATION[@]}"; do
    case "$path" in
      "$entry"*) return 0 ;;
    esac
  done
  return 1
}

packaging=()
deviations=()
count=0
while IFS= read -r path; do
  [[ -n "$path" ]] || continue
  count=$((count + 1))
  if deviation "$path" && ! allowed "$path"; then
    # An upstream code path the fork changed on purpose. Registered, never fatal.
    deviations+=("$path")
    (( list_only )) && printf '%s\n' "$path"
  elif ! allowed "$path"; then
    packaging+=("$path")
    (( list_only )) && printf '%s\n' "$path"
  fi
done <<< "$changed"

if (( list_only )); then
  log "${count} changed path(s)"
  log "deviations from upstream code: ${#deviations[@]}"
  exit 0
fi

if (( ${#packaging[@]} > 0 )); then
  echo "[scope][ERROR] these paths differ from upstream ${base} but are not in the VOS whitelist:" >&2
  printf '  %s\n' "${packaging[@]}" >&2
  echo >&2
  echo "Either move the change into ictrek.app/ docker/ scripts/vos-dev.sh Makefile or" >&2
  echo "docs/vos-integration.md, or add the path to WHITELIST in this script deliberately." >&2
  exit 1
fi

if (( ${#deviations[@]} > 0 )); then
  log "${#deviations[@]} registered deviation(s) in upstream code (kept for the next rebase):"
  printf '  %s\n' "${deviations[@]}"
  log "all ${count} changed path(s) are inside the VOS whitelist or registered as deviations"
else
  log "all ${count} changed path(s) are inside the VOS whitelist"
fi
