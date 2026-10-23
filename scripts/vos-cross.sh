#!/usr/bin/env bash
# Hand a finished image's website to the other architecture, so its wasm is not compiled twice.
#
#   scripts/vos-cross.sh export [tag]   after docker/build_image.sh: copy dist/web out of that image
#   scripts/vos-cross.sh status         is dist/web fresh, complete, and usable by the other host?
#
# The build itself is unchanged on both hosts: docker/build_image.sh, with whatever --no-push /
# --no-feishu / --feishu you already pass. This script only moves its output around; the other host
# needs no wrapper at all, because PHOTOCRAFT_WEB_DIST is build_image.sh's own input:
#
#   # arm host, after the tree syncs
#   cp -a dist/web /tmp/webdist          # a local copy: Docker reads the context through the mount
#   PHOTOCRAFT_BUILD_SHA="$(cat /tmp/webdist/source-revision)" \
#   PHOTOCRAFT_WEB_DIST=/tmp/webdist \
#     bash docker/build_image.sh --no-feishu --no-push
#
# Why: the wasm output is architecture-independent, but docker/Dockerfile.vos compiles it inside
# every image build (fat/thin LTO, codegen-units=1: the workspace's highest peak memory, tens of
# minutes). Exporting it once lets the other architecture build through docker/Dockerfile.runtime,
# which compiles only photocraft-storage — a native binary that genuinely differs per architecture.
#
# The handoff is dist/web plus a `source-revision` stamp, because docker/build_image.sh refuses a
# distribution whose stamp differs from PHOTOCRAFT_BUILD_SHA. `export` also refuses an image whose
# OCI revision label is not HEAD: exporting a stale image would publish old wasm under a new
# version, which the stamp check alone cannot catch (the stamp is written from HEAD here, so it
# would agree with itself while describing the wrong bytes).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${HERE}/.." && pwd)"

IMAGE_REPO="${PHOTOCRAFT_WEB_IMAGE_REPO:-swr.cn-southwest-2.myhuaweicloud.com/ictrek/photocraft-web}"
DIST="${ROOT_DIR}/dist/web"
STAMP="source-revision"

log()  { printf '\033[36m[vos-cross]\033[0m %s\n' "$*"; }
warn() { printf '\033[33m[vos-cross]\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[31m[vos-cross][ERROR]\033[0m %s\n' "$*" >&2; exit 1; }

usage() {
  cat <<'EOF'
Usage: scripts/vos-cross.sh <export|status> [tag]

  export [tag]   after docker/build_image.sh: export dist/web from that image and stamp it with
                 HEAD. The tag defaults to <arch>_<YYYYMMDD> — the tag build_image.sh just used.
  status         report what dist/web holds and whether it is fresh and complete.

Typical cross-architecture release:

  # amd host
  PHOTOCRAFT_BUILD_SHA="$(git rev-parse HEAD)" bash docker/build_image.sh --no-feishu --no-push
  scripts/vos-cross.sh export

  # arm host, after the tree syncs
  cp -a dist/web /tmp/webdist
  PHOTOCRAFT_BUILD_SHA="$(cat /tmp/webdist/source-revision)" \
  PHOTOCRAFT_WEB_DIST=/tmp/webdist \
    bash docker/build_image.sh --no-feishu --no-push
EOF
}

command -v docker >/dev/null || die "missing docker"

head_sha() {
  git -C "${ROOT_DIR}" rev-parse HEAD 2>/dev/null \
    || die "cannot read HEAD in ${ROOT_DIR}; run this from the checkout that owns the commit"
}

host_arch() {
  case "$(uname -m)" in
    x86_64|amd64)  printf 'amd' ;;
    aarch64|arm64) printf 'arm' ;;
    *) die "unsupported host architecture: $(uname -m)" ;;
  esac
}

# `<arch>_<YYYYMMDD>`, the tag docker/build_image.sh assigns (it is deliberately sha-free: the
# revision lives in an OCI label, see docs/vos-integration.md › Versioning).
image_tag() { printf '%s_%s' "$1" "$(date -u +%Y%m%d)"; }

# The image's org.opencontainers.image.revision label, or empty when the image is missing.
image_revision() {
  docker image inspect "$1" --format '{{index .Config.Labels "org.opencontainers.image.revision"}}' 2>/dev/null || true
}

# Refuse to hand on a distribution that does not describe HEAD.
check_revision() {
  local image="$1" want actual
  want="$(head_sha)"
  actual="$(image_revision "${image}")"
  [[ -n "${actual}" ]] || die "${image} does not exist or carries no revision label:
  build it first:
      PHOTOCRAFT_BUILD_SHA=${want} bash docker/build_image.sh --no-feishu --no-push"
  [[ "${actual}" == "${want}" ]] || die "${image} was built from ${actual}, but HEAD is ${want}.
  Exporting it would ship old wasm under a new version. Rebuild first:
      PHOTOCRAFT_BUILD_SHA=${want} bash docker/build_image.sh --no-feishu --no-push"
}

# Copy the finished site out of an image. The image is the source of truth: its builder stage ran
# gzip -9 and scripts/vos-fonts.sh, which a bare `trunk build` does not.
export_web() {
  local image="$1" tmp container="pc-cross-export-$$"
  log "exporting ${image} → dist/web"
  docker image inspect "${image}" >/dev/null 2>&1 || die "no such image: ${image}"
  check_revision "${image}"

  tmp="$(mktemp -d)"
  # shellcheck disable=SC2064  # expand the container name now, not when the trap runs
  trap "docker rm -f '${container}' >/dev/null 2>&1 || true; rm -rf '${tmp}'" EXIT
  docker create --name "${container}" "${image}" >/dev/null
  docker cp "${container}:/usr/share/nginx/html/." "${tmp}/"
  [[ -s "${tmp}/index.html" ]] || die "${image} has no index.html under /usr/share/nginx/html"

  # The runtime image must serve what the full image serves. Both gaps are invisible until a user
  # sees square boxes or a 59 MB uncompressed download, so say so now.
  [[ -d "${tmp}/fonts" ]] || warn "${image} carries no fonts/; CJK labels will render as empty boxes"
  compgen -G "${tmp}/*.gz" >/dev/null || warn "${image} carries no precompressed files; nginx's gzip_static will serve the wasm uncompressed"

  rm -rf "${DIST}"
  mkdir -p "${DIST}"
  cp -a "${tmp}/." "${DIST}/"
  printf '%s' "$(head_sha)" > "${DIST}/${STAMP}"
  log "dist/web ready: $(du -sh "${DIST}" | cut -f1), stamp $(cat "${DIST}/${STAMP}")"
  log "next, on the other host:"
  log "  cp -a dist/web /tmp/webdist"
  log "  PHOTOCRAFT_BUILD_SHA=\$(cat /tmp/webdist/${STAMP}) PHOTOCRAFT_WEB_DIST=/tmp/webdist bash docker/build_image.sh --no-feishu --no-push"
}

cmd_export() {
  local tag="${1:-}"
  [[ $# -gt 0 ]] && shift
  export_web "${IMAGE_REPO}:${tag:-$(image_tag "$(host_arch)")}"
}

cmd_status() {
  local sha stamp image
  sha="$(head_sha)"
  printf 'HEAD            %s\n' "${sha}"
  if [[ ! -d "${DIST}" ]]; then
    printf 'dist/web        missing (build, then: scripts/vos-cross.sh export)\n'
    return 0
  fi
  stamp="$(cat "${DIST}/${STAMP}" 2>/dev/null || true)"
  printf 'dist/web        %s\n' "$(du -sh "${DIST}" | cut -f1)"
  printf '  stamp         %s\n' "${stamp:-<missing: Dockerfile.runtime will refuse it>}"
  printf '  fonts         %s\n' "$([[ -d "${DIST}/fonts" ]] && echo "present ($(ls "${DIST}/fonts" | wc -l) families)" || echo "MISSING: CJK renders as empty boxes")"
  printf '  precompressed %s\n' "$(compgen -G "${DIST}/*.gz" >/dev/null && echo "present ($(compgen -G "${DIST}/*.gz" | wc -l) files)" || echo "MISSING: nginx gzip_static will serve uncompressed")"
  if [[ "${stamp}" == "${sha}" ]]; then
    printf '  verdict       matches HEAD; the other host can build from it\n'
  else
    printf '  verdict       STALE: rebuild, then scripts/vos-cross.sh export\n'
  fi
  image="${IMAGE_REPO}:$(image_tag "$(host_arch)")"
  printf '%s\n  revision      %s\n' "${image}" "$(image_revision "${image}")"
}

case "${1:-}" in
  export) shift; cmd_export "$@" ;;
  status) shift; cmd_status "$@" ;;
  -h|--help|"") usage; exit 0 ;;
  *) usage >&2; die "unknown command: $1" ;;
esac
