#!/usr/bin/env bash
# Release the VOS app package: bump ictrek.app/VERSION, record ictrek.app/UPSTREAM.yml, commit,
# tag vos-photocraft-v<version> and push. GitHub Actions (.github/workflows/vos-release.yml)
# builds the images and publishes the pull-mode package.
#
# With no arguments it follows the upstream core that local main has already merged (the newest
# release tag reachable from main), and increments the build when main has not moved. A tag that is
# only fetched, or only exists upstream, is ignored: sync first, then release. The check is local, so
# a release needs no network.
#
# Version shape: <upstream core>-vos.<build>[+up<base>]
#   0.8.2-vos.1          the first VOS build of upstream 0.8.2
#   0.8.2-vos.2          a second delivery on the same upstream core
#   0.9.0-vos.1          the first build after moving to upstream 0.9.0
#   0.9.0-vos.1+up0.8.2  we raised the core ourselves; the upstream base is still 0.8.2
set -euo pipefail

APP_LABEL="photocraft"
TAG_PREFIX="vos-photocraft-v"
# The branch that mirrors upstream. The upstream core is read from the newest release tag already
# merged into it, so this only changes after a real sync (see docs/vos-integration.md).
UPSTREAM_BRANCH="${VOS_UPSTREAM_BRANCH:-main}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_DIR="$(cd "${HERE}/.." && pwd)"
REPO_ROOT="$(git -C "${APP_DIR}" rev-parse --show-toplevel)"
VERSION_FILE="${APP_DIR}/VERSION"
UPSTREAM_FILE="${APP_DIR}/UPSTREAM.yml"

usage() {
  cat <<'EOF'
Usage:
  ./update_version.sh [--same] [--upstream CORE | --bump [patch|minor|major] | --core CORE [--base CORE]] [--ref SHA]

  (no arguments)          follow the upstream core already merged into local main; if main has not
                          moved since the last release, increment the build instead
                          0.8.2-vos.1 -> 0.8.2-vos.2   (main still on v0.8.2)
                          0.8.2-vos.1 -> 0.9.0-vos.1   (main has v0.9.0 merged)
                          Only tags already merged into local main count: one that was merely
                          fetched, or only exists upstream, is deliberately ignored.
  --same                  force another delivery of the same core, without looking at main
  --upstream CORE         move to a newer upstream core                 --upstream 0.9.0 -> 0.9.0-vos.1
  --bump [part]           we raised the core ourselves (part: default minor)
                          --bump -> 0.9.0-vos.1+up0.8.2  (upstream base 0.8.2 stays recorded)
  --core CORE --base B    the explicit form of --bump (base defaults to the current core)

The upstream ref is read from the local upstream release tag "v<core>" (or an "upstream/v<core>"
tag of our own); pass --ref when the base has neither locally. Commit your code first; this script
refuses a dirty worktree.
EOF
}

die() { echo "[release][ERROR] $*" >&2; exit 1; }

# No arguments: follow the upstream core already merged into local main, and fall back to
# incrementing the build when main has not moved. This is the common case -- a release is either the
# next delivery of the same core or the first after a sync -- and it costs nothing: the check reads
# only local tags. `--same` forces the increment without looking.
mode="from-main"
core=""
base=""
part="minor"
ref=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --base-from-main)
      # Kept as an explicit alias so scripts written earlier keep working.
      mode="from-main"; shift ;;
    --same)
      mode="same"; shift ;;
    --upstream)
      mode="upstream"; core="${2:-}"; shift 2 ;;
    --bump)
      mode="bump"
      if [[ "${2:-}" =~ ^(patch|minor|major)$ ]]; then part="$2"; shift 2; else shift; fi ;;
    --core)
      mode="bump"; core="${2:-}"; shift 2 ;;
    --base)
      base="${2:-}"; shift 2 ;;
    --ref)
      ref="${2:-}"; shift 2 ;;
    patch|minor|major)
      mode="bump"; part="$1"; shift ;;
    -h|--help)
      usage; exit 0 ;;
    *)
      die "unknown argument: $1 (see --help)" ;;
  esac
done

VERSION_RE='^[0-9]+\.[0-9]+\.[0-9]+(-vos\.[0-9]+)?(\+[0-9A-Za-z.-]+)?$'
CORE_RE='^[0-9]+\.[0-9]+\.[0-9]+$'

current="$(tr -d '[:space:]' < "${VERSION_FILE}")"
[[ "${current}" =~ ${VERSION_RE} ]] || die "invalid ictrek.app/VERSION: ${current}"

current_core="${current%%-*}"
current_build=0
if [[ "${current}" == *-vos.* ]]; then
  current_build="${current#*-vos.}"
  current_build="${current_build%%+*}"
fi

# SemVer build metadata (+upX.Y.Z) means we raised the core ourselves, so the real upstream base
# is not current_core. Carry it through a same-core release: dropping it would make UPSTREAM.yml
# claim a base the build does not have, and that value reaches the image labels.
current_base="${current_core}"
current_bumped=false
case "${current}" in
  *+up*) current_base="${current#*+up}"; current_bumped=true ;;
esac

bump_core() {
  local ver="$1" which="$2" major minor patch
  IFS=. read -r major minor patch <<< "${ver}"
  case "${which}" in
    patch) patch=$((patch + 1)) ;;
    minor) minor=$((minor + 1)); patch=0 ;;
    major) major=$((major + 1)); minor=0; patch=0 ;;
  esac
  printf '%s.%s.%s\n' "${major}" "${minor}" "${patch}"
}

record_base=true
case "${mode}" in
  from-main)
    # The core is whatever upstream release local main has actually merged. No network: a tag that
    # is only fetched is ignored on purpose, because recording it would name a base commit this
    # branch does not contain. sort -V is numeric-aware, so 0.9.0 < 0.10.0 (a `${tag//./}` + `-gt`
    # comparison reads "090" as octal and fails on any 8 or 9 in the version).
    newest_tag="$(git -C "${REPO_ROOT}" tag --merged "${UPSTREAM_BRANCH}" -l 'v[0-9]*' \
      --sort=-v:refname 2>/dev/null | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | head -1 || true)"
    [[ -n "${newest_tag}" ]] || die "no release tag (v<core>) is merged into local ${UPSTREAM_BRANCH}
  yet: sync it to an upstream release first (git fetch upstream --tags && git switch ${UPSTREAM_BRANCH} &&
  git merge --ff-only v<new upstream version>), then rebase this branch onto it"
    core="${newest_tag#v}"
    if [[ "${core}" == "${current_core}" ]]; then
      echo "local ${UPSTREAM_BRANCH} is on upstream ${core} (the current core); incrementing the build" >&2
      mode="same"
      core="${current_core}"
      base="${current_base}"
      record_base="${current_bumped}"
    else
      echo "local ${UPSTREAM_BRANCH} has upstream ${core} merged (current core ${current_core}); following it" >&2
      base="${core}"
      record_base=false
    fi
    ;;
  same)
    core="${current_core}"
    base="${current_base}"
    record_base="${current_bumped}"
    ;;
  upstream)
    [[ "${core}" =~ ${CORE_RE} ]] || die "invalid core version: ${core}"
    if [[ "${core}" == "${current_core}" ]]; then
      die "core ${core} is already the current core; run without arguments for another VOS build"
    fi
    base="${core}"
    record_base=false
    ;;
  bump)
    [[ "${core}" =~ ${CORE_RE} ]] || core="$(bump_core "${current_core}" "${part}")"
    # An empty base means "the core we are actually based on now" (which is current_base, not
    # current_core, when the core was already raised once).
    [[ "${base}" =~ ${CORE_RE} ]] || base="${current_base}"
    if [[ "${core}" == "${base}" ]]; then
      die "--bump needs a core above the base (${base}); for another VOS build of the same core run without arguments"
    fi
    ;;
esac

build=1
if [[ "${core}" == "${current_core}" ]]; then
  build=$((current_build + 1))
fi

version="${core}-vos.${build}"
if [[ "${record_base}" == "true" ]]; then
  version="${version}+up${base}"
fi

if [[ -z "${ref}" ]]; then
  # Upstream tags its releases v<core>; upstream/v<core> is the fork's own marker for a base that
  # upstream never tagged (e.g. a plain commit on main). Try both before giving up.
  ref="$(git -C "${REPO_ROOT}" rev-parse --short=9 "v${base}" 2>/dev/null \
      || git -C "${REPO_ROOT}" rev-parse --short=9 "upstream/v${base}" 2>/dev/null \
      || true)"
  [[ -n "${ref}" ]] || die "no ref for upstream ${base}: fetch it (git fetch upstream --tags),
  tag the base yourself (git tag upstream/v${base} <sha>), or pass --ref <sha>"
fi
[[ "${ref}" =~ ^[0-9a-f]{7,40}$ ]] || die "invalid upstream ref: ${ref}"

# The recorded base must be an ancestor of what we ship. Without this, --upstream/--base-from-main
# could name a release this branch does not contain, and UPSTREAM.yml (which verify-scope.sh uses as
# its comparison base) would then describe a tree we do not have.
git -C "${REPO_ROOT}" merge-base --is-ancestor "${ref}" HEAD 2>/dev/null \
  || die "the recorded upstream base ${ref} is not an ancestor of HEAD: sync first
  (git fetch upstream --tags; git switch ${UPSTREAM_BRANCH} && git merge --ff-only v<new upstream version>;
  git switch - && git rebase ${UPSTREAM_BRANCH}), or pass --ref <sha> of a commit you do contain"

# ref and base_sha are different things and must not be conflated:
#   ref      the upstream *release* we follow (the v<core> tag), for provenance: it is what
#            upstream published, and what the image label cites.
#   base_sha the commit this fork actually diverged from -- the merge base with the upstream
#            branch. verify-scope.sh diffs against this, so using the tag here would report every
#            upstream commit since that tag as fork drift (187 of them on 0.8.2).
# They coincide only when the tag happens to be the fork point.
base_sha="$(git -C "${REPO_ROOT}" merge-base HEAD "${UPSTREAM_BRANCH}" 2>/dev/null | cut -c1-9 || true)"
[[ -n "${base_sha}" ]] || base_sha="${ref}"

git -C "${REPO_ROOT}" diff --quiet && git -C "${REPO_ROOT}" diff --cached --quiet \
  || die "worktree is not clean; commit code changes before releasing"

trigger_tag="${TAG_PREFIX}${version}"
public_tag="v${version}"
if git -C "${REPO_ROOT}" ls-remote --exit-code --tags origin "refs/tags/${trigger_tag}" >/dev/null 2>&1; then
  die "release tag already exists on origin: ${trigger_tag}"
fi

{
  echo "# Where this VOS build's core version comes from. Written by scripts/update_version.sh."
  echo "#"
  echo "# core        the upstream PhotoCraft version this build is based on (Cargo.toml"
  echo "#             [workspace.package] version, upstream tag v<core>)"
  echo "# repo/ref    the upstream repository and the release commit the core came from (the v<core> tag;
#             image label provenance)
# base_sha    the commit this fork diverged from (merge-base with the upstream branch), which is
#             what verify-scope.sh diffs against -- not necessarily the tag's commit"
  echo "# fork_build  which VOS build of this core: <core>-vos.<fork_build>"
  echo "# core_bumped true when we raised the core version ourselves, so the upstream base is"
  echo "#             older than the core in VERSION (then core_base names that upstream core"
  echo "#             and VERSION carries it as SemVer build metadata, +up<core_base>)"
  echo "core: \"${core}\""
  echo "repo: storytold/photocraft"
  echo "ref: \"${ref}\""
  echo "base_sha: \"${base_sha}\""
  echo "fork_build: ${build}"
  if [[ "${record_base}" == "true" ]]; then
    echo "core_bumped: true"
    echo "core_base: \"${base}\""
  else
    echo "core_bumped: false"
  fi
} > "${UPSTREAM_FILE}"

printf '%s\n' "${version}" > "${VERSION_FILE}"
git -C "${REPO_ROOT}" add "${VERSION_FILE}" "${UPSTREAM_FILE}"
git -C "${REPO_ROOT}" commit -m "chore: release VOS ${APP_LABEL} ${version}"
git -C "${REPO_ROOT}" tag "${trigger_tag}"
branch="$(git -C "${REPO_ROOT}" branch --show-current)"
git -C "${REPO_ROOT}" push origin "${branch}"
git -C "${REPO_ROOT}" push origin "${trigger_tag}"

echo "Pushed ${trigger_tag}. CI builds the images and publishes ${public_tag};"
echo "image tags: <arch>_<date>_<sha> (Feishu) and ${version}-<arch> (readable)."
