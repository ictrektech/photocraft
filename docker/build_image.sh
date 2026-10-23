#!/usr/bin/env bash
# Build, tag and (optionally) publish the PhotoCraft VOS web image.
#
# Run on a matching build host after rsync, with PHOTOCRAFT_BUILD_SHA supplied from the local checkout.
# Tags are fixed <arch>_<YYYYMMDD>; version/revision are recorded in OCI labels.
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

# The toolchain base image (docker/Dockerfile.toolchain) supplies Rust, the wasm32 target, trunk,
# wasm-bindgen-cli and wasm-opt, so this build compiles none of them. Keep these defaults in sync
# with docker/Dockerfile.toolchain; the tag that identifies the image is computed by
# docker/toolchain_tag.sh, which all three scripts call so they cannot drift apart.
RUST_VERSION="${RUST_VERSION:-1.99}"
TRUNK_VERSION="${TRUNK_VERSION:-0.21.14}"
WASM_BINDGEN_VERSION="${WASM_BINDGEN_VERSION:-0.2.129}"
WASM_OPT_VERSION="${WASM_OPT_VERSION:-version_123}"
CARGO_REGISTRY_MIRROR="${CARGO_REGISTRY_MIRROR:-https://mirrors.tuna.tsinghua.edu.cn/crates.io-index/}"
GITHUB_ACCELERATOR="${GITHUB_ACCELERATOR:-https://gh-proxy.com/}"

ARCH=""
PUSH=1
FEISHU=1
SHEETS=()

log() { echo "[build_image] $*"; }
die() { echo "[build_image][ERROR] $*" >&2; exit 1; }

usage() {
  cat <<'EOF'
Usage: docker/build_image.sh [--arch amd|arm] [--sheet S]... [--no-push] [--no-feishu]

  --arch amd|arm     target architecture (default: the build host's)
  --sheet S          Feishu sheet to record into (default: the sheets for the target architecture)
  --no-push          build and tag locally, do not push
  --no-feishu        do not read credentials or write to the sheet
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --arch) ARCH="${2:-}"; shift 2 ;;
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
[[ -f "${VERSION_FILE}" ]] || die "missing ${VERSION_FILE}"
VERSION="$(tr -d '[:space:]' < "${VERSION_FILE}")"
[[ "${VERSION}" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-vos\.[0-9]+)?(\+[0-9A-Za-z.-]+)?$ ]] \
  || die "invalid ictrek.app/VERSION: ${VERSION}"

# The commit the image was built from. Explicit input wins; otherwise derive it, so a checkout with
# git needs no environment variable at all. Order:
#   1. PHOTOCRAFT_BUILD_SHA         the caller's answer, as before
#   2. the web distribution's stamp  reuse path: the wasm was compiled elsewhere, so that build's
#                                    commit is the only honest answer (the stamp is checked below)
#   3. git rev-parse HEAD            a plain checkout
# A build host fed over rsync has no git, which is why (1) and (2) come first; it only reaches (3)
# when it has both a .git and no distribution, i.e. it is building the wasm itself.
GIT_SHA="${PHOTOCRAFT_BUILD_SHA:-}"
if [[ -z "${GIT_SHA}" && -n "${PHOTOCRAFT_WEB_DIST:-}" && -f "${PHOTOCRAFT_WEB_DIST}/source-revision" ]]; then
  GIT_SHA="$(tr -d '[:space:]' < "${PHOTOCRAFT_WEB_DIST}/source-revision")"
  log "revision ${GIT_SHA} (from the web distribution)"
fi
if [[ -z "${GIT_SHA}" ]]; then
  GIT_SHA="$(git -C "${ROOT_DIR}" rev-parse HEAD 2>/dev/null || true)"
  [[ -n "${GIT_SHA}" ]] && log "revision ${GIT_SHA} (from git)"
fi
[[ "${GIT_SHA}" =~ ^[0-9a-f]{7,40}$ ]] || die "cannot tell which commit this build is from:
  not a git checkout (a build host fed over rsync has no .git), and no PHOTOCRAFT_WEB_DIST stamp.
  Pass the commit the tree was copied from:
      PHOTOCRAFT_BUILD_SHA=<sha> docker/build_image.sh"
BUILD_DATE="$(date -u +%Y%m%d)"
BUILD_ISO="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
MAIN_TAG="${ARCH}_${BUILD_DATE}"
case "${ARCH}" in
  amd) DOCKER_ARCH="amd64" ;;
  arm) DOCKER_ARCH="arm64" ;;
esac
MAIN_IMAGE="${IMAGE_REPO}:${MAIN_TAG}"

TOOLCHAIN_REPO="${PHOTOCRAFT_TOOLCHAIN_IMAGE_REPO:-swr.cn-southwest-2.myhuaweicloud.com/ictrek/photocraft-toolchain}"
TOOLCHAIN_TAG="$(RUST_VERSION="${RUST_VERSION}" \
                 TRUNK_VERSION="${TRUNK_VERSION}" \
                 WASM_BINDGEN_VERSION="${WASM_BINDGEN_VERSION}" \
                 WASM_OPT_VERSION="${WASM_OPT_VERSION}" \
                 CARGO_REGISTRY_MIRROR="${CARGO_REGISTRY_MIRROR}" \
                 GITHUB_ACCELERATOR="${GITHUB_ACCELERATOR}" \
                 bash "${ROOT_DIR}/docker/toolchain_tag.sh" "${DOCKER_ARCH}")"
TOOLCHAIN_IMAGE="${TOOLCHAIN_REPO}:${TOOLCHAIN_TAG}"

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
log "tags ${MAIN_TAG}"
log "toolchain ${TOOLCHAIN_IMAGE}"

# Fail before a long build when the toolchain for this architecture is not available: the build
# would otherwise stop with an opaque "failed to resolve source metadata" pull error. Accepts a
# locally tagged image too, which is what a --no-push docker/build_toolchain.sh leaves behind.
if ! docker image inspect "${TOOLCHAIN_IMAGE}" >/dev/null 2>&1 \
   && ! docker manifest inspect "${TOOLCHAIN_IMAGE}" >/dev/null 2>&1; then
  die "toolchain image ${TOOLCHAIN_IMAGE} is neither local nor in the registry.
  Build and publish it once (on an ${DOCKER_ARCH} host -- SWR rejects multi-arch index pushes):
      RUST_VERSION=${RUST_VERSION} TRUNK_VERSION=${TRUNK_VERSION} \\
        WASM_BINDGEN_VERSION=${WASM_BINDGEN_VERSION} WASM_OPT_VERSION=${WASM_OPT_VERSION} \\
        docker/build_toolchain.sh"
fi

# A second architecture can reuse the exact verified WASM output instead of compiling it twice.
WEB_DIST="${PHOTOCRAFT_WEB_DIST:-}"
if [[ -n "${WEB_DIST}" ]]; then
  [[ -s "${WEB_DIST}/index.html" && -f "${WEB_DIST}/source-revision" ]] || die "missing verified web distribution"
  [[ "$(cat "${WEB_DIST}/source-revision")" == "${GIT_SHA}" ]] || die "web distribution revision differs from PHOTOCRAFT_BUILD_SHA"
  DOCKERFILE="docker/Dockerfile.runtime"
else
# Copies the craft-fonts `fonts/` folder into the build context, where docker/Dockerfile.vos hands
# it to scripts/vos-fonts.sh. The copy is a real directory, since Docker leaves a symlinked one out
# of the context. The path is git-ignored.
CRAFT_FONTS_DEFAULT="${ROOT_DIR}/../craft-fonts"
CRAFT_FONTS_SRC="${CRAFT_FONTS_DIR:-${CRAFT_FONTS_DEFAULT}}"
FONTS_DIR="${ROOT_DIR}/craft-fonts"

# Leaves the build context as it was found, on failure too.
rm -rf "${FONTS_DIR}"
trap 'rm -rf "${FONTS_DIR}"' EXIT

[[ -f "${CRAFT_FONTS_SRC}/fonts/manifest.txt" ]] || die "no craft-fonts checkout at ${CRAFT_FONTS_SRC}:
  the image would serve no CJK fonts, so every Chinese, Japanese and Korean label renders as an empty box.
  Clone it next to this repository (or point CRAFT_FONTS_DIR at an existing checkout):
      git clone https://github.com/storytold/craft-fonts ${CRAFT_FONTS_DEFAULT}"
mkdir -p "${FONTS_DIR}"
cp -r "${CRAFT_FONTS_SRC}/fonts" "${FONTS_DIR}/" || die "copying ${CRAFT_FONTS_SRC}/fonts failed"
log "fonts from ${CRAFT_FONTS_SRC} ($(du -sh "${FONTS_DIR}/fonts" | cut -f1))"

fi

build_args=(
  --file "${DOCKERFILE}"
  --build-arg "TOOLCHAIN_IMAGE=${TOOLCHAIN_IMAGE}"
  --build-arg "PHOTOCRAFT_BUILD_SHA=${GIT_SHA}"
  --build-arg "PHOTOCRAFT_BUILD_DATE=$(date -u +%F)"
  --build-arg "PHOTOCRAFT_VOS_VERSION=${VERSION}"
  --build-arg "PHOTOCRAFT_UPSTREAM=${UPSTREAM_LABEL}"
  --label "org.opencontainers.image.version=${VERSION}"
  --label "org.opencontainers.image.revision=${GIT_SHA}"
  --label "org.opencontainers.image.created=${BUILD_ISO}"
  --label "com.ictrek.photocraft.upstream=${UPSTREAM_LABEL}"
  -t "${MAIN_IMAGE}"
)

if [[ -n "${WEB_DIST}" ]]; then
  ( cd "${ROOT_DIR}" && docker buildx build --platform "linux/${DOCKER_ARCH}" --load --provenance=false --sbom=false --network host --build-context "webdist=${WEB_DIST}" "${build_args[@]}" . )
elif [[ "${ARCH}" == "${host_arch}" ]]; then
  # Native build: the plain builder is enough and reuses the local layer cache.
  ( cd "${ROOT_DIR}" && docker build --network host "${build_args[@]}" . )
else
  # Cross-architecture build (an amd64 host producing the arm64 image): buildx with qemu.
  docker buildx version >/dev/null 2>&1 || die "cross-building ${ARCH} needs \`docker buildx\` (and binfmt/qemu)"
  log "cross-building with buildx --platform linux/${DOCKER_ARCH}"
  ( cd "${ROOT_DIR}" && docker buildx build --platform "linux/${DOCKER_ARCH}" --load --provenance=false --sbom=false --network host "${build_args[@]}" . )
fi

if (( PUSH )); then
  log "pushing ${MAIN_IMAGE}"
  docker push "${MAIN_IMAGE}"
else
  log "--no-push: ${MAIN_IMAGE} stays local"
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
  ${MAIN_IMAGE}

Next: record ${MAIN_TAG} in the Feishu sheet for each platform (if --no-feishu was used), then
package with ictrek.app/scripts/update_version.sh and let CI publish.
EOF
