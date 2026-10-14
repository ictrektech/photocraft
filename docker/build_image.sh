#!/usr/bin/env bash
# Build, tag and (optionally) publish the PhotoCraft VOS web image.
#
#   docker/build_image.sh                       # host architecture, push, record in Feishu
#   docker/build_image.sh --arch arm            # arm64 (needs buildx + qemu on an amd64 host)
#   docker/build_image.sh --no-push --no-feishu # local build only
#   docker/build_image.sh --tag-suffix r2       # second build of the same commit on the same day
#
# Tags, following the VAI convention (see ../nowen-video/build_image.sh):
#   main tag     <arch>_<YYYYMMDD>_<short sha>     what the Feishu sheet records and package.sh reads
#   release tag  <VERSION>-<amd64|arm64>           human-readable, e.g. 0.8.2-vos.1-amd64
#
# The version is NOT part of the main tag on purpose: the tag says which commit and when, the
# release tag says which release, and the image labels say which upstream base.
set -euo pipefail

REGISTRY="${REGISTRY:-swr.cn-southwest-2.myhuaweicloud.com}"
NAMESPACE="${NAMESPACE:-ictrek}"
IMAGE_REPO="${PHOTOCRAFT_WEB_IMAGE_REPO:-${REGISTRY}/${NAMESPACE}/photocraft-web}"
COMPONENT="photocraft-web"

SPREADSHEET_TOKEN="${FEISHU_SPREADSHEET_TOKEN:-Htotsn3oahO1zxt73YMcaB1zn8e}"
FEISHU_CONFIG_FILE="${FEISHU_CONFIG_FILE:-${HOME}/.feishu.components.json}"
FEISHU_FALLBACK_CONFIG_FILE="${FEISHU_FALLBACK_CONFIG_FILE:-${HOME}/.feishu.json}"

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION_FILE="${ROOT_DIR}/ictrek.app/VERSION"
UPSTREAM_FILE="${ROOT_DIR}/ictrek.app/UPSTREAM.yml"
DOCKERFILE="docker/Dockerfile.vos"

ARCH=""
PUSH=1
FEISHU=1
TAG_SUFFIX=""
SHEETS=()

log() { echo "[build_image] $*"; }
die() { echo "[build_image][ERROR] $*" >&2; exit 1; }

usage() {
  cat <<'EOF'
Usage: docker/build_image.sh [--arch amd|arm] [--tag-suffix S] [--sheet S]... [--no-push] [--no-feishu]

  --arch amd|arm     target architecture (default: the build host's)
  --tag-suffix S     append -S to the main tag (a second build of the same commit the same day)
  --sheet S          Feishu sheet to record into (default: the sheets for the target architecture)
  --no-push          build and tag locally, do not push
  --no-feishu        do not read credentials or write to the sheet
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --arch) ARCH="${2:-}"; shift 2 ;;
    --tag-suffix) TAG_SUFFIX="${2:-}"; shift 2 ;;
    --sheet) SHEETS+=("${2:-}"); shift 2 ;;
    --no-push) PUSH=0; shift ;;
    --push) PUSH=1; shift ;;
    --no-feishu) FEISHU=0; shift ;;
    --feishu) FEISHU=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown argument: $1 (see --help)" ;;
  esac
done

command -v docker >/dev/null || die "missing docker"
command -v python3 >/dev/null || die "missing python3"

host_arch="$(uname -m)"
case "${host_arch}" in
  x86_64|amd64) host_arch="amd" ;;
  aarch64|arm64) host_arch="arm" ;;
  *) die "unsupported build host architecture: ${host_arch}" ;;
esac
ARCH="${ARCH:-${host_arch}}"
case "${ARCH}" in
  amd|arm) ;;
  *) die "--arch must be amd or arm" ;;
esac
case "${TAG_SUFFIX}" in
  ""|r[0-9]*) ;;
  *) die "--tag-suffix must look like r2" ;;
esac

[[ -f "${VERSION_FILE}" ]] || die "missing ${VERSION_FILE}"
VERSION="$(tr -d '[:space:]' < "${VERSION_FILE}")"
[[ "${VERSION}" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-vos\.[0-9]+)?(\+[0-9A-Za-z.-]+)?$ ]] \
  || die "invalid ictrek.app/VERSION: ${VERSION}"

GIT_SHA="$(git -C "${ROOT_DIR}" rev-parse --short=9 HEAD 2>/dev/null)" || die "not a git repository"
BUILD_DATE="$(date -u +%Y%m%d)"
BUILD_ISO="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
MAIN_TAG="${ARCH}_${BUILD_DATE}_${GIT_SHA}"
[[ -n "${TAG_SUFFIX}" ]] && MAIN_TAG="${MAIN_TAG}-${TAG_SUFFIX}"
case "${ARCH}" in
  amd) DOCKER_ARCH="amd64" ;;
  arm) DOCKER_ARCH="arm64" ;;
esac
# A Docker tag allows [A-Za-z0-9_.-] only, so SemVer build metadata (+up0.8.2, set when we raised
# the core ourselves) has to come off here. It is not lost: the full version stays in the
# org.opencontainers.image.version label and in ictrek.app/UPSTREAM.yml.
TAG_VERSION="${VERSION%%+*}"
RELEASE_TAG="${TAG_VERSION}-${DOCKER_ARCH}"
MAIN_IMAGE="${IMAGE_REPO}:${MAIN_TAG}"
RELEASE_IMAGE="${IMAGE_REPO}:${RELEASE_TAG}"

# Upstream provenance for the image labels (ictrek.app/UPSTREAM.yml).
UPSTREAM_CORE=""
UPSTREAM_REF=""
if [[ -f "${UPSTREAM_FILE}" ]]; then
  UPSTREAM_CORE="$(python3 - "${UPSTREAM_FILE}" <<'PY'
import sys
for line in open(sys.argv[1], encoding="utf-8"):
    if line.startswith("core:"):
        print(line.split(":", 1)[1].strip().strip('"'))
        break
PY
)"
  UPSTREAM_REF="$(python3 - "${UPSTREAM_FILE}" <<'PY'
import sys
for line in open(sys.argv[1], encoding="utf-8"):
    if line.startswith("ref:"):
        print(line.split(":", 1)[1].strip().strip('"'))
        break
PY
)"
fi
UPSTREAM_LABEL="storytold/photocraft@${UPSTREAM_REF:-unknown}"

log "version ${VERSION} (upstream ${UPSTREAM_CORE:-unknown} @ ${UPSTREAM_REF:-unknown})"
log "arch ${ARCH} (${DOCKER_ARCH}) host ${host_arch}"
log "tags ${MAIN_TAG} and ${RELEASE_TAG}"

build_args=(
  --file "${DOCKERFILE}"
  --build-arg "PHOTOCRAFT_BUILD_SHA=${GIT_SHA}"
  --build-arg "PHOTOCRAFT_BUILD_DATE=$(date -u +%F)"
  --build-arg "PHOTOCRAFT_VOS_VERSION=${VERSION}"
  --build-arg "PHOTOCRAFT_UPSTREAM=${UPSTREAM_LABEL}"
  --label "org.opencontainers.image.version=${VERSION}"
  --label "org.opencontainers.image.revision=${GIT_SHA}"
  --label "org.opencontainers.image.created=${BUILD_ISO}"
  --label "com.ictrek.photocraft.upstream=${UPSTREAM_LABEL}"
  -t "${MAIN_IMAGE}"
  -t "${RELEASE_IMAGE}"
)

if [[ "${ARCH}" == "${host_arch}" ]]; then
  # Native build: the plain builder is enough and reuses the local layer cache.
  ( cd "${ROOT_DIR}" && docker build "${build_args[@]}" . )
else
  # Cross-architecture build (an amd64 host producing the arm64 image): buildx with qemu.
  docker buildx version >/dev/null 2>&1 || die "cross-building ${ARCH} needs \`docker buildx\` (and binfmt/qemu)"
  log "cross-building with buildx --platform linux/${DOCKER_ARCH}"
  ( cd "${ROOT_DIR}" && docker buildx build --platform "linux/${DOCKER_ARCH}" --load "${build_args[@]}" . )
fi

if (( PUSH )); then
  for image in "${MAIN_IMAGE}" "${RELEASE_IMAGE}"; do
    log "pushing ${image}"
    docker push "${image}"
  done
else
  log "--no-push: ${MAIN_IMAGE} and ${RELEASE_IMAGE} stay local"
fi

if (( FEISHU )); then
  if [[ ${#SHEETS[@]} -eq 0 ]]; then
    if [[ "${ARCH}" == "amd" ]]; then
      SHEETS=("AMD_with_mxn100" "AMD_with_cuda")
    else
      SHEETS=("ARM_without_cuda" "ARM_with_cuda" "l4t" "thor_spark" "SOPHON_bm1688")
    fi
  fi
  SHEETS_JOINED="$(printf '%s,' "${SHEETS[@]}")"
  SHEETS_JOINED="${SHEETS_JOINED%,}"
  FEISHU_SPREADSHEET_TOKEN="${SPREADSHEET_TOKEN}" \
  FEISHU_CONFIG_FILE="${FEISHU_CONFIG_FILE}" \
  FEISHU_FALLBACK_CONFIG_FILE="${FEISHU_FALLBACK_CONFIG_FILE}" \
  IMAGE_REPO="${IMAGE_REPO}" \
    python3 "${ROOT_DIR}/docker/feishu_record.py" \
      --component "${COMPONENT}" --tag "${MAIN_TAG}" --sheets "${SHEETS_JOINED}" --date "${BUILD_DATE}"
else
  log "--no-feishu: ${MAIN_TAG} not recorded; package.sh will not see it"
fi

cat <<EOF

Built:
  main     ${MAIN_IMAGE}
  release  ${RELEASE_IMAGE}

Next: record ${MAIN_TAG} in the Feishu sheet for each platform (if --no-feishu was used), then
package with ictrek.app/scripts/update_version.sh and let CI publish.
EOF
