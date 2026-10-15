#!/usr/bin/env bash
# Build and publish the VOS web-build toolchain base image (docker/Dockerfile.toolchain).
#
# The application image (docker/Dockerfile.vos) starts FROM this image, so the ~400-crate
# `cargo install trunk` / `cargo install wasm-bindgen-cli` step is paid once per toolchain version
# instead of on every application build. Run this when RUST_VERSION, TRUNK_VERSION or
# WASM_BINDGEN_VERSION changes -- docker/build_image.sh does not build it.
#
#   docker/build_toolchain.sh                    # build + push the image for this host's arch
#   docker/build_toolchain.sh --no-push          # build and tag locally only
#   docker/build_toolchain.sh --force            # rebuild even when the tag is already published
#   docker/build_toolchain.sh --print-tag        # print the tag docker/Dockerfile.vos will pull
#
# Build NATIVELY on each architecture (an amd64 host for amd64, an arm64 host for arm64): Huawei SWR
# rejects OCI multi-arch index pushes (HTTP 400), so each arch carries its own tag, and compiling
# Rust under qemu is impractically slow. Run it on the arm64 build host for the arm64 tag.
set -euo pipefail

REGISTRY="${REGISTRY:-swr.cn-southwest-2.myhuaweicloud.com}"
NAMESPACE="${NAMESPACE:-ictrek}"
IMAGE_REPO="${PHOTOCRAFT_TOOLCHAIN_IMAGE_REPO:-${REGISTRY}/${NAMESPACE}/photocraft-toolchain}"

# Keep these in sync with docker/Dockerfile.toolchain. The script passes them explicitly so the tag
# and the image contents cannot drift apart.
RUST_VERSION="${RUST_VERSION:-1.99}"
TRUNK_VERSION="${TRUNK_VERSION:-0.21.14}"
WASM_BINDGEN_VERSION="${WASM_BINDGEN_VERSION:-0.2.129}"
# Must match the wasm-opt version trunk downloads for its own build (docker/Dockerfile.toolchain).
WASM_OPT_VERSION="${WASM_OPT_VERSION:-version_123}"
CARGO_REGISTRY_MIRROR="${CARGO_REGISTRY_MIRROR:-https://mirrors.tuna.tsinghua.edu.cn/crates.io-index/}"
# github.com is unreachable from the build network, but two things need it: the craft-libs git
# dependency (root Cargo.toml) that `cargo metadata` must clone, and the wasm-opt release archive.
GITHUB_ACCELERATOR="${GITHUB_ACCELERATOR:-https://gh-proxy.com/}"

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DOCKERFILE="docker/Dockerfile.toolchain"

FORCE=0
PUSH=1
PRINT_TAG=0

log() { echo "[build_toolchain] $*"; }
die() { echo "[build_toolchain][ERROR] $*" >&2; exit 1; }

usage() {
  cat <<'EOF'
Usage: docker/build_toolchain.sh [--force] [--no-push] [--print-tag]

  --force        rebuild even when the tag already exists in the registry
  --no-push      build and tag locally, do not push
  --print-tag    print the image tag the application build will pull, then exit

Environment: REGISTRY, NAMESPACE, RUST_VERSION, TRUNK_VERSION, WASM_BINDGEN_VERSION,
             WASM_OPT_VERSION, CARGO_REGISTRY_MIRROR, GITHUB_ACCELERATOR

Every one of those inputs is part of the tag (docker/toolchain_tag.sh), so changing any of them
produces a new tag and this script rebuilds instead of reporting "already published".
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --force) FORCE=1; shift ;;
    --no-push) PUSH=0; shift ;;
    --push) PUSH=1; shift ;;
    --print-tag) PRINT_TAG=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown argument: $1 (see --help)" ;;
  esac
done

case "$(uname -m)" in
  x86_64|amd64) DOCKER_ARCH="amd64" ;;
  aarch64|arm64) DOCKER_ARCH="arm64" ;;
  *) die "unsupported build host architecture: $(uname -m)" ;;
esac

# The tag encodes every input that changes the image contents, so a stale tag can never be
# mistaken for a current one. One implementation, shared with the scripts that consume the image
# (docker/build_image.sh, scripts/vos-dev.sh), so the producer and consumers cannot drift.
TAG="$(RUST_VERSION="${RUST_VERSION}" \
       TRUNK_VERSION="${TRUNK_VERSION}" \
       WASM_BINDGEN_VERSION="${WASM_BINDGEN_VERSION}" \
       WASM_OPT_VERSION="${WASM_OPT_VERSION}" \
       CARGO_REGISTRY_MIRROR="${CARGO_REGISTRY_MIRROR}" \
       GITHUB_ACCELERATOR="${GITHUB_ACCELERATOR}" \
       bash "${ROOT_DIR}/docker/toolchain_tag.sh" "${DOCKER_ARCH}")"
IMAGE="${IMAGE_REPO}:${TAG}"

if (( PRINT_TAG )); then
  printf '%s\n' "${IMAGE}"
  exit 0
fi

command -v docker >/dev/null || die "missing docker"

log "toolchain ${IMAGE}"
log "rust ${RUST_VERSION}, trunk ${TRUNK_VERSION}, wasm-bindgen-cli ${WASM_BINDGEN_VERSION}, mirror ${CARGO_REGISTRY_MIRROR}"
log "wasm-opt ${WASM_OPT_VERSION}, github accelerator ${GITHUB_ACCELERATOR}"

# Skip the multi-minute build when the exact toolchain is already published: the tag fully
# determines the contents, so an existing tag is never stale.
if (( FORCE == 0 )) && docker manifest inspect "${IMAGE}" >/dev/null 2>&1; then
  log "already published, nothing to do (use --force to rebuild)"
  exit 0
fi

log "building (this compiles trunk and wasm-bindgen-cli; several minutes on a cold cache)"
docker build \
  --file "${DOCKERFILE}" \
  --build-arg "RUST_VERSION=${RUST_VERSION}" \
  --build-arg "TRUNK_VERSION=${TRUNK_VERSION}" \
  --build-arg "WASM_BINDGEN_VERSION=${WASM_BINDGEN_VERSION}" \
  --build-arg "CARGO_REGISTRY_MIRROR=${CARGO_REGISTRY_MIRROR}" \
  --build-arg "WASM_OPT_VERSION=${WASM_OPT_VERSION}" \
  --build-arg "GITHUB_ACCELERATOR=${GITHUB_ACCELERATOR}" \
  --build-arg "TOOLCHAIN_TAG=${TAG}" \
  --label "org.opencontainers.image.version=${TAG}" \
  -t "${IMAGE}" \
  "${ROOT_DIR}"

if (( PUSH )); then
  log "pushing ${IMAGE}"
  docker push "${IMAGE}"
else
  log "--no-push: ${IMAGE} stays local"
fi

cat <<EOF

Toolchain ready: ${IMAGE}

The application build (docker/build_image.sh) starts FROM this tag. On the other architecture, run
this script on that host first: SWR rejects multi-arch index pushes, so each arch has its own tag.
EOF
