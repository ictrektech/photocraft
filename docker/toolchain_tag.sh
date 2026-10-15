#!/usr/bin/env bash
# The one place the toolchain image tag is computed.
#
#   docker/toolchain_tag.sh [arch]      # arch defaults to the host's (amd64 | arm64)
#
# docker/build_toolchain.sh (which builds and publishes the image), docker/build_image.sh and
# scripts/vos-dev.sh (which start FROM it) all call this, so the tag can never drift between the
# producer and the consumers.
#
# The tag encodes EVERY input that changes the image contents, not just the tool versions:
#
#   rust<ver>-trunk<ver>-wasm-bindgen<ver>-wasm-opt<ver>-<fingerprint>-<arch>
#
# The versions are spelled out because they are readable and bumped by hand. Everything else -- the
# two network knobs (CARGO_REGISTRY_MIRROR, GITHUB_ACCELERATOR) and the recipe itself
# (Dockerfile.toolchain) -- goes into one short hash, because the knobs are URLs a Docker tag cannot
# hold and the recipe is large.
#
# All three belong in the tag because all three are baked into the image (config.toml, git's
# insteadOf, and the instructions). Without them the "already published, nothing to do" check in
# build_toolchain.sh would skip a rebuild and hand back a stale image -- which is how a mirror, an
# accelerator and a Dockerfile change were each silently ignored, all three within one session.
set -euo pipefail

RUST_VERSION="${RUST_VERSION:-1.99}"
TRUNK_VERSION="${TRUNK_VERSION:-0.21.14}"
WASM_BINDGEN_VERSION="${WASM_BINDGEN_VERSION:-0.2.129}"
# Must match the wasm-opt version trunk downloads for its own build (see Dockerfile.toolchain).
# The `version_` prefix is trunk's spelling; the tag drops it, so the tag reads wasm-opt123.
WASM_OPT_VERSION="${WASM_OPT_VERSION:-version_123}"
CARGO_REGISTRY_MIRROR="${CARGO_REGISTRY_MIRROR:-https://mirrors.tuna.tsinghua.edu.cn/crates.io-index/}"
GITHUB_ACCELERATOR="${GITHUB_ACCELERATOR:-https://gh-proxy.com/}"

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RECIPE_FILE="${TOOLCHAIN_RECIPE_FILE:-${here}/Dockerfile.toolchain}"

arch="${1:-}"
if [ -z "${arch}" ]; then
  case "$(uname -m)" in
    x86_64|amd64) arch=amd64 ;;
    aarch64|arm64) arch=arm64 ;;
    *) echo "unsupported architecture: $(uname -m)" >&2; exit 1 ;;
  esac
fi

# sha256sum is GNU coreutils (Linux); shasum is the BSD/macOS spelling. Both are available
# somewhere on every host this repo builds on.
sha8() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum | cut -c1-8
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 | cut -c1-8
  else
    # cksum is weaker but present everywhere; the fingerprint only needs to change when inputs do.
    cksum | tr -d ' ' | cut -c1-8
  fi
}

[ -r "${RECIPE_FILE}" ] || { echo "cannot read the toolchain recipe: ${RECIPE_FILE}" >&2; exit 1; }
# Only the instructions count: comments and blank lines are not part of the image, so a comment-only
# edit must not mint a new tag (and a duplicate registry entry for identical bytes).
recipe="$(grep -vE '^[[:space:]]*(#|$)' "${RECIPE_FILE}" || true)"

fingerprint="$(printf '%s|%s|%s' \
  "${CARGO_REGISTRY_MIRROR}" "${GITHUB_ACCELERATOR}" "${recipe}" | sha8)"

printf 'rust%s-trunk%s-wasm-bindgen%s-wasm-opt%s-%s-%s\n' \
  "${RUST_VERSION}" "${TRUNK_VERSION}" "${WASM_BINDGEN_VERSION}" \
  "${WASM_OPT_VERSION#version_}" \
  "${fingerprint}" "${arch}"
