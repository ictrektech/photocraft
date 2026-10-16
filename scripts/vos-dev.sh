#!/usr/bin/env bash
# VOS local development for PhotoCraft: serve the app from this workstation inside the platform.
#
#   make vos-dev                 # trunk dev server (watch + auto reload) + temporary Traefik route
#   VOS_DEV_WEB=0 make vos-dev   # fidelity mode: dist/web served by the VOS image's nginx
#   make vos-dev-down            # remove the route, stop the server/container
#   make vos-dev-status          # what is running and who the platform traffic goes to
#   make vos-dev-logs            # follow the fidelity container
#   make vos-dev-web             # (re)build dist/web, with fonts, without the platform
#
# Nothing here touches the app the platform has installed: it writes one Traefik file-provider
# route that outranks the installed app's route and removes it again on the way out. The platform
# reconciles installed apps (docker compose up) every 60s, so never stop those containers by hand.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

# Reuse the workspace Cargo build cache.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${ROOT_DIR}/target}"

# Keep Cargo incremental compilation enabled for the dev server.
export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-1}"

APP_ID="com.ictrek.photocraft"
BASE_PATH="/app/${APP_ID}"
DEV_PORT="${VOS_DEV_PORT:-38092}"           # trunk dev server
DIST_PORT="${VOS_DEV_DIST_PORT:-38093}"     # fidelity-mode nginx
HOST_GW="${VOS_HOST_GW_IP:-172.17.0.1}"     # Traefik reaches the host through this gateway
# Where the dev server listens. NOT loopback: the Traefik container reaches this machine through the
# docker bridge (HOST_GW), and a 127.0.0.1-only listener is invisible from there — the platform then
# answers "Bad Gateway" while the port looks perfectly healthy on the host. 0.0.0.0 matches the
# sibling VOS apps (vite --host); with the default VOS networks nothing outside this machine reaches
# the port.
DEV_BIND_ADDR="${VOS_DEV_BIND_ADDR:-0.0.0.0}"
TRAEFIK_CONTAINER="${VOS_TRAEFIK_CONTAINER:-traefik}"
TRAEFIK_CONF_DIR="${VOS_TRAEFIK_CONF_DIR:-/etc/traefik/dynamic-conf}"
ROUTE_FILE="${TRAEFIK_CONF_DIR}/${APP_ID}.dev.yml"
TMPL="docker/vos-dev/traefik.dev.yml.tmpl"
# Lives next to index.html: trunk resolves the build target, dist and hook cwd against the config
# file's directory (see the file's header).
TRUNK_CONFIG="apps/photocraft-web/Trunk.vos-dev.toml"
COMPOSE_FILE="docker/vos-dev/docker-compose.yml"
CONTAINER="photocraft-vos-dev-web"
# The toolchain base image docker/Dockerfile.vos starts FROM (docker/Dockerfile.toolchain): Rust,
# the wasm32 target, trunk and wasm-bindgen-cli. Keep these defaults in sync with
# docker/build_toolchain.sh, which computes the same tag. VOS_DEV_TOOLCHAIN_IMAGE overrides the
# whole reference, for a locally tagged toolchain that is not pushed yet.
TOOLCHAIN_REPO="${VOS_DEV_TOOLCHAIN_REPO:-swr.cn-southwest-2.myhuaweicloud.com/ictrek/photocraft-toolchain}"
RUST_VERSION="${RUST_VERSION:-1.99}"
TRUNK_VERSION="${TRUNK_VERSION:-0.21.14}"
WASM_BINDGEN_VERSION="${WASM_BINDGEN_VERSION:-0.2.129}"
WASM_OPT_VERSION="${WASM_OPT_VERSION:-version_123}"
CARGO_REGISTRY_MIRROR="${CARGO_REGISTRY_MIRROR:-https://mirrors.tuna.tsinghua.edu.cn/crates.io-index/}"
GITHUB_ACCELERATOR="${GITHUB_ACCELERATOR:-https://gh-proxy.com/}"
TOOLCHAIN_IMAGE="${VOS_DEV_TOOLCHAIN_IMAGE:-}"
# Generous by default: the first build compiles 2000+ crates and trunk may download wasm-bindgen
# (8 MB) or wasm-opt (114 MB) at a few tens of KB/s. Ctrl-C is the escape hatch, not a timeout.
HEALTH_TIMEOUT="${VOS_DEV_HEALTH_TIMEOUT:-1800}"
WEB_IMAGE="${VOS_DEV_WEB_IMAGE:-nginx:1.28-alpine}"
STATE_DIR="${ROOT_DIR}/dist/vos-dev"
TRUNK_LOG="${STATE_DIR}/trunk.log"
TRUNK_PID_FILE="${STATE_DIR}/trunk.pid"
TRUNK_PID=""   # the live `trunk serve` process while the dev server runs (see cmd_up)
TAIL_PID=""    # the `tail -f` that mirrors trunk's output to this terminal
DOWN_DONE=0    # set by cmd_down so the INT/TERM/EXIT traps clean up only once
PLATFORM_GW="${VOS_PLATFORM_GW:-http://172.17.0.1:1182}"   # Traefik web-internal entrypoint (HTTP)

log()  { printf '\033[36m[vos-dev]\033[0m %s\n' "$*"; }
warn() { printf '\033[33m[vos-dev] %s\033[0m\n' "$*" >&2; }
die()  { printf '\033[31m[vos-dev][ERROR]\033[0m %s\n' "$*" >&2; exit 1; }

use_trunk() { [ "${VOS_DEV_WEB:-1}" = "1" ]; }
# The toolchain tag encodes every input that changes the image contents (including the network
# knobs), exactly as docker/build_toolchain.sh computes it -- both call docker/toolchain_tag.sh.
# TARGETARCH in the Dockerfile covers the architecture.
toolchain_image() {
  if [ -n "${TOOLCHAIN_IMAGE}" ]; then
    printf '%s' "${TOOLCHAIN_IMAGE}"
    return 0
  fi
  local arch
  case "$(uname -m)" in
    x86_64|amd64) arch=amd64 ;;
    aarch64|arm64) arch=arm64 ;;
    *) arch="$(uname -m)" ;;
  esac
  printf '%s:%s' "${TOOLCHAIN_REPO}" \
    "$(RUST_VERSION="${RUST_VERSION}" \
       TRUNK_VERSION="${TRUNK_VERSION}" \
       WASM_BINDGEN_VERSION="${WASM_BINDGEN_VERSION}" \
       WASM_OPT_VERSION="${WASM_OPT_VERSION}" \
       CARGO_REGISTRY_MIRROR="${CARGO_REGISTRY_MIRROR}" \
       GITHUB_ACCELERATOR="${GITHUB_ACCELERATOR}" \
       bash "${ROOT_DIR}/docker/toolchain_tag.sh" "${arch}")"
}
web_port() { if use_trunk; then printf '%s' "$DEV_PORT"; else printf '%s' "$DIST_PORT"; fi; }
have() { command -v "$1" >/dev/null 2>&1; }
port_in_use() { (exec 3<>"/dev/tcp/127.0.0.1/$1") 2>/dev/null; }

ensure_port_free() {
  if port_in_use "$1"; then
    die "port $1 is already in use (a stale trunk or container?).
  Free it, or pick another port: VOS_DEV_PORT=38094 make vos-dev (or VOS_DEV_DIST_PORT=38095)."
  fi
}

# The pid file holds trunk itself, not the subshell that started it (see cmd_up): the subshell
# exits at once with a process substitution, which is what made `wait` return immediately and the
# script tear down a perfectly healthy dev server.
pid_alive() { kill -0 "$1" 2>/dev/null; }
tree_running() { [ -f "$TRUNK_PID_FILE" ] && pid_alive "$(cat "$TRUNK_PID_FILE")"; }
pid_in_file() { cat "$TRUNK_PID_FILE" 2>/dev/null || printf ''; }


# ------------------------------------------------------------------------------ Traefik route

route_on() {
  local port="$1" tmp
  if ! docker ps --format '{{.Names}}' 2>/dev/null | grep -qx "$TRAEFIK_CONTAINER"; then
    warn "no container named ${TRAEFIK_CONTAINER}: the temporary route was NOT written."
    warn "Open http://127.0.0.1:${port}${BASE_PATH}/?v=dev directly, or set VOS_TRAEFIK_CONTAINER."
    return 0
  fi
  # /data/vos/traefik belongs to the container user, so the file is copied in instead of being
  # written through the bind mount.
  tmp="$(mktemp)"
  sed -e "s|__HOST__|${HOST_GW}|g" -e "s|__PORT__|${port}|g" "$TMPL" > "$tmp"
  chmod 644 "$tmp"
  if ! docker cp "$tmp" "${TRAEFIK_CONTAINER}:${ROUTE_FILE}" >/dev/null; then
    rm -f "$tmp"
    die "could not copy the route into ${TRAEFIK_CONTAINER}:${ROUTE_FILE}"
  fi
  rm -f "$tmp"
  log "route written: ${ROUTE_FILE} -> http://${HOST_GW}:${port}"
}

# URL the platform's own gateway will request. The dev server has to answer it for the app to work
# in the browser; trunk serves at the root, the fidelity container under BASE_PATH.
dev_probe_url() {
  local port="$1"
  if use_trunk; then
    printf 'http://%s:%s/index.html' "$HOST_GW" "$port"
  else
    printf 'http://%s:%s%s/index.html' "$HOST_GW" "$port" "$BASE_PATH"
  fi
}

# A wildcard bind is not a sane URL to hand out (0.0.0.0 works on Linux, not everywhere).
local_addr() {
  case "$DEV_BIND_ADDR" in
    0.0.0.0) printf '127.0.0.1' ;;
    *) printf '%s' "$DEV_BIND_ADDR" ;;
  esac
}

# A dev server bound to loopback answers on the host and is a 502 through Traefik (that is exactly
# how "Bad Gateway" happens). One probe, used by both the ready banner and `make vos-dev-status`.
traefik_reachable() {
  local port="$1"
  docker ps --format '{{.Names}}' 2>/dev/null | grep -qx "$TRAEFIK_CONTAINER" || return 1
  docker exec "$TRAEFIK_CONTAINER" sh -c "wget -q -O /dev/null -T 5 '$(dev_probe_url "$port")'" 2>/dev/null
}

verify_reachable_from_traefik() {
  local port="$1"
  if traefik_reachable "$port"; then
    log "reachable from ${TRAEFIK_CONTAINER} ($(dev_probe_url "$port"))"
    return 0
  fi
  warn "the dev server does NOT answer $(dev_probe_url "$port") from inside ${TRAEFIK_CONTAINER}, so"
  warn "the platform will show 'Bad Gateway' even though the port looks healthy here. It is bound to"
  warn "${DEV_BIND_ADDR}. Fix: VOS_DEV_BIND_ADDR=${HOST_GW} make vos-dev   (or VOS_DEV_BIND_ADDR=0.0.0.0)"
}

route_off() {
  if docker ps --format '{{.Names}}' 2>/dev/null | grep -qx "$TRAEFIK_CONTAINER"; then
    docker exec "$TRAEFIK_CONTAINER" rm -f "$ROUTE_FILE" >/dev/null 2>&1 || true
    log "route removed: ${ROUTE_FILE} (the platform serves its installed app again)"
  fi
}

# Wait until the dev server answers with the site. While it does not, report progress and surface
# the first build error instead of sitting silent until the timeout (trunk keeps serving 404s after a
# failed build, so "the port answers" is not enough).
wait_for_http() {
  local url="$1" what="$2" waited=0 code="" hinted=0 last=""
  log "waiting for ${what} (the first build takes minutes; the build output is above)"
  while (( waited < HEALTH_TIMEOUT )); do
    code="$(curl -o /dev/null -s -w '%{http_code}' --max-time 3 "${url}" 2>/dev/null || true)"
    if [ "${code}" = "200" ]; then
      log "${what} is up (${url})"
      return 0
    fi
    if (( ! hinted )) && [ -f "${TRUNK_LOG}" ] && grep -qE "can't find crate|error\[E[0-9]+\]|^error:|❌" "${TRUNK_LOG}"; then
      hinted=1
      warn "the build is failing; first error from ${TRUNK_LOG}:"
      grep -m1 -E "can't find crate for|error\[E[0-9]+\]|^error:" "${TRUNK_LOG}" | sed 's/^/    /' >&2
      if grep -q "may not be installed" "${TRUNK_LOG}"; then
        warn "that is a missing Rust target; fix it with: rustup target add wasm32-unknown-unknown"
      fi
      warn "waiting anyway (${TRUNK_LOG} has the rest); Ctrl-C stops the dev server"
    fi
    sleep 2
    waited=$((waited + 2))
    if (( waited % 15 == 0 )); then
      # Say what trunk is busy with (downloading a tool, optimising) instead of only echoing the last
      # line: those steps take minutes on a slow link and look like a hang otherwise.
      step="$(grep -E "downloading |Compiling |optimizing|wasm-opt" "${TRUNK_LOG}" 2>/dev/null | tail -n 1 | tr -d '\r' | cut -c1-120)"
      last="$(tail -n 1 "${TRUNK_LOG}" 2>/dev/null | tr -d '\r' | cut -c1-120)"
      log "still waiting (${waited}s of ${HEALTH_TIMEOUT}s; HTTP ${code:-none}): ${step:-${last:-<no output yet>}}"
    fi
  done
  die "${what} did not answer ${url} within ${HEALTH_TIMEOUT}s; see ${TRUNK_LOG}"
}

# Compiling the browser build needs the wasm target. Catching it here turns a two-minute silent
# build failure into one line with the fix. Skip with VOS_DEV_SKIP_TARGET_CHECK=1 when the toolchain
# comes from somewhere else.
preflight_build_tools() {
  [ "${VOS_DEV_SKIP_TOOL_CHECK:-0}" = "1" ] && return 0
  if command -v rustup >/dev/null 2>&1 && ! rustup target list --installed 2>/dev/null | grep -qx wasm32-unknown-unknown; then
    die "the wasm32-unknown-unknown Rust target is not installed, so trunk cannot build the browser
  app. Install it once (this needs to write to ~/.rustup):
      rustup target add wasm32-unknown-unknown
  Behind a firewall this also helps:
      RUSTUP_DIST_SERVER=https://mirrors.tuna.tsinghua.edu.cn/rustup rustup target add wasm32-unknown-unknown
  Or build the site in Docker instead (no host Rust toolchain at all):
      VOS_DEV_WEB=0 make vos-dev
  To skip this check: VOS_DEV_SKIP_TOOL_CHECK=1"
  fi
  # wasm-bindgen: needed in both modes. Without it on PATH, trunk downloads its own copy on first
  # use (about 8 MB; minutes on a slow link). The version must match the wasm-bindgen pin in
  # Cargo.lock, or trunk ignores the system binary and downloads one anyway.
  local want
  want="$(grep -A1 "^name = \\"wasm-bindgen\\"$" Cargo.lock 2>/dev/null | grep -m1 '^version' | cut -d'"' -f2 || true)"
  if [ -n "${want}" ] && ! command -v wasm-bindgen >/dev/null 2>&1; then
    warn "wasm-bindgen is not on PATH, so trunk will download its own build of one (about 8 MB)."
    warn "The faster route (what the upstream Docker build also does):"
    warn "    cargo install wasm-bindgen-cli --version ${want} --locked"
  fi
}

# --------------------------------------------------------------------------------------- fonts

fonts_src() {
  if [ -n "${PHOTOCRAFT_WEB_FONTS_DIR:-}" ]; then
    printf '%s' "$PHOTOCRAFT_WEB_FONTS_DIR"
  elif [ -n "${CRAFT_FONTS_DIR:-}" ] && [ -f "${CRAFT_FONTS_DIR}/fonts/manifest.txt" ]; then
    printf '%s' "$CRAFT_FONTS_DIR"
  fi
}

# Copy the served fonts into a built site (dist/web, or a container's copy of it). A missing font
# folder is a warning: the site still works, it just has no extra CJK coverage. Trunk mode does the
# same thing during the build through the post_build hook in apps/photocraft-web/Trunk.vos-dev.toml.
copy_fonts() {
  local dest="$1" src
  src="$(fonts_src)"
  if [ -z "$src" ] || [ ! -f "${src}/fonts/manifest.txt" ]; then
    warn "no craft-fonts found (set CRAFT_FONTS_DIR or PHOTOCRAFT_WEB_FONTS_DIR): the site serves no extra fonts"
    return 0
  fi
  log "copying the font manifest and its files from ${src}"
  PHOTOCRAFT_WEB_FONTS_DIR="$src" bash "${ROOT_DIR}/scripts/vos-fonts.sh" "$dest"
}

# -------------------------------------------------------------------------- build the site

# The same build the deployed image runs, without publishing anything: the Dockerfile's builder
# stage with a local output, so it needs Docker but no host Rust/trunk toolchain. It does need the
# toolchain base image (docker/Dockerfile.toolchain), which supplies Rust, trunk and wasm-bindgen.
build_web() {
  log "building dist/web through the image build stage (docker/Dockerfile.vos --target builder)"
  if ! docker image inspect "$(toolchain_image)" >/dev/null 2>&1 \
     && ! docker manifest inspect "$(toolchain_image)" >/dev/null 2>&1; then
    die "the toolchain image is not available, so this build has no Rust/trunk/wasm-bindgen:
  $(toolchain_image)
  Build and publish it once (several minutes; do it on this host's architecture):
      docker/build_toolchain.sh
  Then retry: make vos-dev-web"
  fi
  rm -rf dist/vos-dev/web-out
  DOCKER_BUILDKIT=1 docker build \
    --file docker/Dockerfile.vos \
    --target builder \
    --build-arg "TOOLCHAIN_IMAGE=$(toolchain_image)" \
    --output "type=local,dest=dist/vos-dev/web-out" \
    . || die "the web build failed"
  rm -rf dist/web
  mkdir -p dist/web
  cp -R dist/vos-dev/web-out/src/dist/web/. dist/web/
  rm -rf dist/vos-dev/web-out
  [ -s dist/web/index.html ] || die "dist/web/index.html is missing after the build"
  copy_fonts dist/web
}

# ------------------------------------------------------------------------------------ modes

cmd_up() {
  local port
  port="$(web_port)"
  mkdir -p "$STATE_DIR"

  if use_trunk; then
    have trunk || die "trunk is not installed (cargo install trunk --locked); or use VOS_DEV_WEB=0 make vos-dev"
    tree_running && die "a trunk dev server is already running (pid $(pid_in_file)); make vos-dev-down first"
    ensure_port_free "$port"
    preflight_build_tools
    log "starting trunk serve on ${DEV_BIND_ADDR}:${port} (watch + auto reload)"
    log "the platform reaches it as http://${HOST_GW}:${port}; binding to loopback instead would make
  every request through Traefik a 502"
    log "config: ${TRUNK_CONFIG}; its post_build hook re-copies the served fonts after every rebuild"
    # trunk resolves the build target, dist and hook cwd against the config file's directory, so it
    # runs from here just as well — and running it in this shell (no subshell, no pipeline) makes its
    # pid exactly $!, which is what the exit check and cmd_down use. `( ... & )` or a `| tee` both
    # hand $! to a helper that exits at once, which is what used to tear the dev server down
    # immediately after starting it.
    nohup trunk serve --config "$TRUNK_CONFIG" --address "$DEV_BIND_ADDR" --port "$port" > "$TRUNK_LOG" 2>&1 &
    TRUNK_PID=$!
    printf '%s\n' "$TRUNK_PID" > "$TRUNK_PID_FILE"
    log "trunk serve is pid ${TRUNK_PID}"
    # The same output here and in the file (trunk writes the file, this mirrors it to the terminal).
    tail -n +1 -f "$TRUNK_LOG" &
    TAIL_PID=$!
    printf '%s\n' "$TAIL_PID" > "${STATE_DIR}/tail.pid"
    # Probed through HOST_GW (not 127.0.0.1) on purpose: "ready" must mean ready for Traefik.
    wait_for_http "$(dev_probe_url "$port")" "trunk dev server"
  else
    build_web
    ensure_port_free "$port"
    # The published image strips the `listen [::]:8080` line at build time (docker/Dockerfile.vos):
    # on a host whose kernel has no IPv6 that bind fails and nginx exits. Fidelity mode mounts the
    # config directly, so the same line has to come off here or the container dies with
    # "socket() [::]:8080 failed". Doing it to a copy also keeps the mounted file byte-identical to
    # the one the image serves, which is the whole point of this mode.
    sed '/listen \[::\]:8080;/d' packaging/web/nginx.conf > "${STATE_DIR}/nginx.conf"
    if grep -q 'listen \[::\]:8080;' "${STATE_DIR}/nginx.conf"; then
      die "could not strip the IPv6 listener from packaging/web/nginx.conf; the fidelity container would not start"
    fi
    sed -e "s|__VOS_DEV_WEB_IMAGE__|${WEB_IMAGE}|g" -e "s|__VOS_DEV_DIST_PORT__|${port}|g" \
      "$COMPOSE_FILE" > "${STATE_DIR}/docker-compose.rendered.yml"
    log "starting the fidelity container on ${port} (nginx serving dist/web)"
    docker compose -f "${STATE_DIR}/docker-compose.rendered.yml" up -d
    wait_for_http "http://127.0.0.1:${port}${BASE_PATH}/index.html" "fidelity container"
  fi

  route_on "$port"
  verify_reachable_from_traefik "$port"
  local direct_url
  if use_trunk; then
    # trunk serves the built site at the URL root; only the platform route strips BASE_PATH.
    direct_url="http://$(local_addr):${port}/?v=dev"
  else
    direct_url="http://$(local_addr):${port}${BASE_PATH}/?v=dev"
  fi
  cat <<EOF

PhotoCraft dev is ready:
  direct     ${direct_url}
  platform   ${PLATFORM_GW}${BASE_PATH}/?v=dev
  check      curl -sI ${PLATFORM_GW}${BASE_PATH}/ | grep -i x-photocraft-dev

Open the platform and click PhotoCraft: it now loads this working copy.
EOF

  if use_trunk; then
    log "trunk is serving (its output is above); Ctrl-C removes the temporary route and stops it"
    # INT/TERM/HUP for the usual Ctrl-C and kill paths; EXIT so a shell that dies some other way
    # still removes the temporary route (a stale route would keep sending the platform to a dev
    # server that is no longer running).
    trap 'cmd_down "interrupted"' INT TERM HUP
    trap 'cmd_down "shell exiting"' EXIT
    # Stay attached to trunk: rebuilds keep printing here. The loop re-checks liveness instead of
    # relying on `wait`, because trunk may not be a direct child of this shell.
    # trunk is a direct child of this shell (see the launch above), so `wait` is the right primitive:
    # it cannot be fooled by a recycled pid the way a `kill -0` poll can, and a signal runs the trap
    # first, leaving this to return once the trap has stopped trunk.
    local status=0
    wait "$TRUNK_PID" || status=$?
    cmd_down "trunk exited (status ${status})"
  else
    log "fidelity container logs (Ctrl-C leaves it running; stop it with make vos-dev-down)"
    trap 'cmd_down "shell exiting"' EXIT
    docker logs -f "$CONTAINER" 2>/dev/null || true
  fi
}

cmd_down() {
  local reason="${1:-stopped by this shell}"
  # Idempotent: cmd_down runs from the foreground path and, as a safety net, from the EXIT trap, and
  # both the Ctrl-C path and a dying shell must leave the route and the pids cleaned up exactly once.
  if [ "${DOWN_DONE:-0}" = "1" ]; then
    # A signal trap already did the work; still drop the state files so nothing looks "running".
    rm -f "$TRUNK_PID_FILE" "${STATE_DIR}/tail.pid"
    return 0
  fi
  DOWN_DONE=1
  trap - INT TERM HUP EXIT
  route_off
  if [ -f "$TRUNK_PID_FILE" ]; then
    local pid
    pid="$(pid_in_file)"
    if pid_alive "$pid"; then
      # SIGINT first (trunk shuts its server down cleanly), TERM then KILL for a stuck one.
      kill -INT "$pid" 2>/dev/null || true
      local waited=0
      while pid_alive "$pid" && (( waited < 5 )); do sleep 1; waited=$((waited + 1)); done
      if pid_alive "$pid"; then
        kill "$pid" 2>/dev/null || true
        sleep 1
      fi
      pid_alive "$pid" && kill -9 "$pid" 2>/dev/null || true
      log "trunk dev server stopped (pid ${pid}; ${reason})"
    else
      log "trunk is no longer running (${reason})"
    fi
  fi
  # The log follower outlives trunk otherwise and keeps the terminal attached.
  if [ -f "${STATE_DIR}/tail.pid" ]; then
    local tail_pid
    tail_pid="$(cat "${STATE_DIR}/tail.pid")"
    pid_alive "$tail_pid" && kill "$tail_pid" 2>/dev/null || true
    rm -f "${STATE_DIR}/tail.pid"
  fi
  TRUNK_PID=""
  rm -f "$TRUNK_PID_FILE" "${STATE_DIR}/tail.pid"
  if docker ps -a --format '{{.Names}}' 2>/dev/null | grep -qx "$CONTAINER"; then
    docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
    log "fidelity container removed"
  fi
  log "dev environment stopped"
}

cmd_status() {
  local port routing
  port="$(web_port)"
  printf 'mode       : %s\n' "$(use_trunk && echo 'trunk dev server (watch + reload)' || echo 'fidelity container (dist/web)')"
  printf 'port       : %s (%s)\n' "$port" "$(port_in_use "$port" && echo listening || echo 'not listening')"
  printf 'bind addr  : %s (the platform reaches it as %s)\n' "$DEV_BIND_ADDR" "$HOST_GW"
  if tree_running; then printf 'trunk pid  : %s\n' "$(cat "$TRUNK_PID_FILE")"; fi
  if docker ps --format '{{.Names}}' 2>/dev/null | grep -qx "$CONTAINER"; then printf 'container  : running\n'; fi
  if docker ps --format '{{.Names}}' 2>/dev/null | grep -qx "$TRAEFIK_CONTAINER"; then
    if docker exec "$TRAEFIK_CONTAINER" test -f "$ROUTE_FILE" 2>/dev/null; then
      printf 'dev route  : present (%s)\n' "$ROUTE_FILE"
    else
      printf 'dev route  : absent (the platform serves its installed app)\n'
    fi
  else
    printf 'traefik    : container %s not found\n' "$TRAEFIK_CONTAINER"
  fi
  if docker ps --format '{{.Names}}' 2>/dev/null | grep -qx "$TRAEFIK_CONTAINER"; then
    if traefik_reachable "$port"; then
      printf 'from traefik: reachable (http://%s:%s)\n' "$HOST_GW" "$port"
    else
      printf 'from traefik: NOT reachable (http://%s:%s) -> the platform shows Bad Gateway\n' "$HOST_GW" "$port"
    fi
  fi
  if routing="$(curl -sI --max-time 4 "${PLATFORM_GW}${BASE_PATH}/" 2>/dev/null)"; then
    if printf '%s' "$routing" | grep -qi 'x-photocraft-dev'; then
      printf 'platform   : served by THIS workstation (X-Photocraft-Dev present)\n'
    else
      printf 'platform   : served by the installed app (no X-Photocraft-Dev)\n'
    fi
  else
    printf 'platform   : the gateway at %s did not answer\n' "$PLATFORM_GW"
  fi
}

case "${1:-up}" in
  up) cmd_up ;;
  down) cmd_down ;;
  status) cmd_status ;;
  logs) docker logs -f "$CONTAINER" ;;
  route-off) route_off ;;
  route-on) route_on "$(web_port)" ;;
  build-web) build_web ;;
  *) die "unknown command: $1 (up|down|status|logs|route-on|route-off|build-web)" ;;
esac
