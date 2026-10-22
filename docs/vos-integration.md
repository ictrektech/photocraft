# PhotoCraft in VOS

PhotoCraft ships to VOS as a **web application**: the same Rust engine and egui shell compiled to
WebAssembly, served as static files by nginx, embedded in the platform through the usual same-origin
iframe. There is no backend and no per-user state in this first release.

- App id: `com.ictrek.photocraft`
- Platform route: `/app/com.ictrek.photocraft/` (menu entry from `ictrek.app/src/routers.yml`)
- Delivery definition: [`ictrek.app/`](../ictrek.app/README.md)
- Release steps: [`docs/releasing.md`](releasing.md) › VOS application

## What the package contains

```
ictrek.app/src/manifest.yml       app id, name, profiles (amd, arm), OAuth2 client (phase 2), frontend basePath
ictrek.app/src/docker-compose.yml one nginx service per architecture profile, Traefik labels
ictrek.app/src/routers.yml        the platform sidebar entry (flat page, keep-alive, embedded)
ictrek.app/src/README.{zh-CN,en}.md  the App Store listing, rendered with the current version
ictrek.app/scripts/package.sh     renders everything and builds dist/photocraft_<version>_pull.tar
ictrek.app/scripts/audit-package.sh  independent check of the built tarball
```

The image is built from `docker/Dockerfile.vos`, which is the upstream web recipe with one change:
the toolchain comes from the prebuilt base image instead of a GitHub release download, so it builds
on networks where that download is blocked. It carries OCI labels naming the upstream base commit.

### The toolchain base image

`docker/Dockerfile.vos` starts `FROM` a separate toolchain image built by `docker/build_toolchain.sh`
(`docker/Dockerfile.toolchain`): Rust, the `wasm32-unknown-unknown` target, trunk, `wasm-bindgen-cli`
and `wasm-opt`, plus the crates.io mirror and the GitHub rewrite.

It is separate because those two `cargo install` steps compile roughly 400 crates and take many
minutes. In the application Dockerfile they sat above `COPY . .`, so changing *any* earlier ARG —
the mirror URL, the Rust version, the jobs count — invalidated them and the next build paid the
whole cost again. In their own image they are paid once per toolchain version and then pulled.

```bash
docker/build_toolchain.sh                 # build + push the toolchain for this host's architecture
docker/build_toolchain.sh --print-tag     # the tag docker/Dockerfile.vos will pull
docker/build_toolchain.sh --force         # rebuild (e.g. after bumping a version)
```

The tag encodes every input that changes the contents — the tool versions, the network knobs and the
recipe itself — so an existing tag is never stale and the script skips the build when it is already
published.
`docker/build_image.sh` and `make vos-dev-web` both fail early with the exact command to run when
the toolchain is missing, instead of stopping on an opaque pull error.

`docker/toolchain_tag.sh` computes that tag, and `docker/build_toolchain.sh`,
`docker/build_image.sh` and `scripts/vos-dev.sh` all call it, so the producer and the consumers
cannot drift apart:

```
rust<ver>-trunk<ver>-wasm-bindgen<ver>-wasm-opt<ver>-<fingerprint>-<arch>
```

The versions are spelled out; everything else goes into a short sha256 fingerprint — the two
network knobs (`CARGO_REGISTRY_MIRROR`, `GITHUB_ACCELERATOR`), which are URLs a Docker tag cannot
hold, **and `Dockerfile.toolchain` itself**. All three belong in the tag because all three are baked
into the image (`config.toml`, git's `insteadOf`, and the instructions): without them, changing a
mirror, an accelerator or the recipe would leave the tag unchanged and `build_toolchain.sh` would
report "already published" while handing back a stale image. Comments and blank lines are stripped
before hashing, so a comment-only edit does not mint a new tag.

**Build the toolchain natively on each architecture** (an amd64 host for the amd64 tag, an arm64 host
for the arm64 tag): Huawei SWR rejects OCI multi-arch index pushes (HTTP 400), so each arch carries
its own tag, and compiling Rust under qemu is impractically slow.

Bump a version in **three** places, which must agree — `docker/Dockerfile.toolchain`,
`docker/toolchain_tag.sh` (the defaults there feed all three scripts) and
`docker/Dockerfile.vos` (the tag formula in its `FROM` fallback) — then rebuild the toolchain.
`RUST_VERSION` must also stay ≥ the workspace `rust-version` in `Cargo.toml`, `TRUNK_VERSION` must
match `.github/workflows/ci.yml` / `release.yml`, and `WASM_BINDGEN_VERSION` must match the
`wasm-bindgen` pin in `Cargo.lock`.

`WASM_OPT_VERSION` is the exception that is easy to miss: it must equal the wasm-opt version trunk
itself wants, which moves with `TRUNK_VERSION`. trunk compares the string `wasm-opt --version`
prints against it and falls back to downloading when they differ, so a mismatch reintroduces the
blocked GitHub download. `trunk tools show` prints the expected value as `Default Version`; the
image build prints the same table at the end, which is where a mismatch shows up as
`Installed Version: n/a`.

### Why github.com still appears

Two things need GitHub, and neither is reachable from the build network, so both go through
`GITHUB_ACCELERATOR` (default `https://gh-proxy.com/`):

- **the `craft-libs` git dependency** (`Cargo.toml`), which `cargo metadata` clones as the first
  step of `trunk build`. This needs `git` in the image plus `git-fetch-with-cli = true`: cargo
  defaults to libgit2, which reads neither git's `insteadOf` config nor a proxy from the
  environment.
- **the `wasm-opt` release archive**, installed at toolchain-build time so `trunk build --release`
  finds it on `PATH` and never downloads it.

Everything else (Rust, trunk, wasm-bindgen-cli, crates.io) goes through the Tsinghua mirrors.
`GITHUB_ACCELERATOR` is part of the tag, so switching accelerator produces a new tag and a rebuild.

## Local development inside the platform

```bash
make vos-dev                 # nginx fronting the dev build on 38093 + a temporary Traefik route
VOS_DEV_WEB=2 make vos-dev   # trunk dev server only, no container, on 38092
VOS_DEV_WEB=0 make vos-dev   # fidelity mode: dist/web through the VOS image's nginx on 38093
make vos-dev-status          # mode, port, route, and which copy the platform is serving
make vos-dev-down            # remove the route and stop everything
```

while a dev server is up, the platform's PhotoCraft entry loads your working copy instead of the
installed app; `X-Photocraft-Dev: 1` on any response from that route says so:

```bash
curl -sI http://172.17.0.1:1182/app/com.ictrek.photocraft/ | grep -i x-photocraft-dev
```

The temporary route strips `/app/com.ictrek.photocraft` exactly like the installed app's route does,
so every mode serves the same URLs the shipped nginx configuration expects.

**The dev server must not listen on loopback.** Traefik reaches this workstation through the docker
bridge (`VOS_HOST_GW_IP`, `172.17.0.1`), so a `127.0.0.1`-only listener is unreachable from it: the
host sees a healthy port while the platform answers `502 Bad Gateway`. The script therefore binds
`VOS_DEV_BIND_ADDR` (default `0.0.0.0`, matching the sibling VOS apps' `vite --host`);
`VOS_DEV_BIND_ADDR=172.17.0.1` restricts it to the bridge. Both the readiness probe and
`make vos-dev-status` (`from traefik: reachable`) check the URL the platform actually requests, and a
failing check prints the fix instead of leaving you with a blank 502.

The script never touches the installed app's containers (the platform reconciles them every 60
seconds); it only adds and removes `/etc/traefik/dynamic-conf/com.ictrek.photocraft.dev.yml`.

**When the platform shows its own copy, nothing is broken — check `make vos-dev-status` first.** Two
ways to end up there, both now handled by the script:

- The route file is written only once the dev server answers, and removed on exit. Ctrl-C during the
  first build (minutes long) used to leave `trunk serve` running with the route already gone, so the
  platform served the installed app while the next `make vos-dev` died on "port already in use". The
  cleanup traps are now installed before anything starts, and `vos-dev-down` / the next `up` stop a
  leftover trunk whose working directory is **this** checkout (a trunk from another worktree on the
  same port is reported, not killed).
- `NO_COLOR=1` (common in dotfiles and CI images) makes trunk 0.21 exit immediately — it reads the
  variable as the `--no-color` flag, which only accepts `true`/`false`. The script normalises it
  before launching trunk, so `make vos-dev` works with it set.
- A **cached** build makes the script exit 1 in silence, 15 s in, with no `still waiting` line. The
  progress branch in `wait_for_http` reads the build log through `grep`, and a cached build has no
  `Compiling` line, so grep finds nothing; under `set -euo pipefail` that non-match propagated and
  killed the script. Every such substitution now ends in `|| true`. The symptom to recognise:
  `📦 starting build` then `Finished wasm-dev profile … in 0.3s`, then the route is removed and the
  shell exits 1. If you see it after editing this script, check for a substitution without
  `|| true` before assuming trunk failed.

If the page still looks like the installed app, compare the hashed bundle name:

```bash
curl -sk https://127.0.0.1:1180/app/com.ictrek.photocraft/ | grep -o 'photocraft-web-[a-f0-9]*' | head -1
ls dist/web-dev/photocraft-web-*_bg.wasm    # the build the dev server serves
```

**Fonts.** The web build embeds no CJK fonts; the site fetches `fonts/manifest.txt` next to
`index.html` (see `crates/ui-egui/src/served_fonts.rs`). Point `CRAFT_FONTS_DIR` (or
`PHOTOCRAFT_WEB_FONTS_DIR`) at a craft-fonts checkout and they are served; without one the page
shows no CJK glyphs and the script says so:

```bash
CRAFT_FONTS_DIR=../craft-fonts make vos-dev
```

**The build output stays in your terminal.** `make vos-dev` starts trunk in the background writing
`dist/vos-dev/trunk.log` and mirrors that file to the terminal with `tail -f`, so the compile is
watchable in place while the script stays in control of the lifecycle. While it waits for the first
page it reports progress every 15s (including `downloading …` / `Compiling …` steps, which can last
minutes on a slow link) and, as soon as the log shows a build error, prints that error and — for the
common missing-target case — the exact command that fixes it, instead of sitting silent until the
timeout. The default timeout is 30 minutes, not a hang detector: Ctrl-C is the escape hatch.

Quitting is one Ctrl-C, and any other death of the script (a `kill`, a closed terminal) is covered
too: an `EXIT` trap removes the temporary route and stops trunk and the log follower, so the platform
never points at a dev server that has gone away. `make vos-dev-down` does the same by hand and is
harmless when nothing is running.

A missing `wasm32-unknown-unknown` target, and the tools trunk would otherwise download, are checked
up front, before trunk starts:

```
[vos-dev][ERROR] the wasm32-unknown-unknown Rust target is not installed, so trunk cannot build the
  app. Install it once (this needs to write to ~/.rustup):
      rustup target add wasm32-unknown-unknown
  ...
  Or build the site in Docker instead (no host Rust toolchain at all):
      VOS_DEV_WEB=0 make vos-dev
```

Trunk replaces its whole dist directory on every rebuild, so the fonts are re-added after each one
by the `post_build` hook in `apps/photocraft-web/Trunk.vos-dev.toml` (which `make vos-dev` passes
with `--config`; the upstream `Trunk.toml` is untouched). That file has to sit next to `index.html`:
trunk resolves `target`, `dist` and the hook's working directory relative to the **config file's**
directory, so a config kept elsewhere fails with "error getting the canonical path to the build
target HTML file". A manifest listing a file that is not there fails the build rather than quietly
serving fewer fonts.

**You need the wasm target.** Both dev modes compile on this workstation (only fidelity builds in
Docker), so the toolchain needs it once:

```bash
rustup target add wasm32-unknown-unknown
```

The default mode also needs Docker: it serves `dist/web-dev` through an nginx container that mounts
the app's own `packaging/web/nginx.conf`, so the runtime under test is the one that ships. Trunk
still does the building on the host, which is what keeps the ~16s rebuild; the container proxies
trunk's auto-reload websocket back to it. Run `VOS_DEV_WEB=2 make vos-dev` to skip the container
entirely.

Trunk substitutes `{{__TRUNK_ADDRESS__}}` / `{{__TRUNK_WS_BASE__}}` in `index.html` per request, not
on disk, so the file nginx serves would still hold them and the browser would open a websocket to a
host literally named `{{__TRUNK_ADDRESS__}}`. The `post_build` hook (`vos-dev.sh compress`, via
`cmd_wsbase`) rewrites them to `window.location.host` and `/app/com.ictrek.photocraft/` — the
platform's only reachable path — and the dev config gains a matching second websocket location that
rewrites onto trunk's own route.

The tools, and which mode needs which:

| Tool | dev (`make vos-dev`, `VOS_DEV_WEB=2`) | fidelity (`VOS_DEV_WEB=0`) |
| --- | --- | --- |
| `wasm32-unknown-unknown` target | needed | needs only Docker |
| Docker | needed by `make vos-dev`; not by `VOS_DEV_WEB=2` | needed |
| `wasm-bindgen` (0.2.129) | needed; `cargo install wasm-bindgen-cli --version 0.2.129 --locked` avoids trunk's own 8 MB download | inside the image |
| `wasm-opt` / binaryen (114 MB) | **not needed** — trunk skips it outside a release build | inside the image, installed at toolchain-build time through the accelerator |

`scripts/vos-dev.sh` checks the trunk and `wasm-bindgen` versions before starting: `index.html`
selects the Cargo profile with `data-cargo-profile-release` / `-dev`, which a trunk too old to read
ignores, so cargo would fall back to the default `dev` profile instead of `wasm-dev`.
`VOS_DEV_SKIP_TOOL_CHECK=1` bypasses both checks.

Fidelity mode needs no host Rust toolchain at all: `make vos-dev-web` builds `dist/web` through the
image's build stage, which needs only Docker — and it warns first if `wasm-opt` is missing, with the
resumable download commands for a slow link. It does need the toolchain base image, so run
`docker/build_toolchain.sh` once first (see below).

## Building the image and the package

```bash
# Once per toolchain version, on each architecture (see "The toolchain base image"):
docker/build_toolchain.sh

docker/build_image.sh                        # host architecture: build, push, record in Feishu
docker/build_image.sh --arch arm             # arm64, cross-built with buildx
docker/build_image.sh --no-push --no-feishu  # local check

make vos-package          # dry-run package: placeholder image tags, no Feishu, template check
make vos-package-release  # real package: reads the image tags from Feishu, builds the pull tar
make vos-audit PKG=ictrek.app/dist/photocraft_0.8.2-vos.1_pull.tar
```

The package is pull mode: one outer tar containing only `app.tar.gz`, whose `.env` names the two
registry images. Nothing embeds an image, so the package stays a few kilobytes.

## Versioning

`ictrek.app/VERSION` is `<upstream core>-vos.<n>[+up<base>]`, e.g. `0.8.2-vos.1`: the first VOS
build of upstream PhotoCraft 0.8.2. `ictrek.app/UPSTREAM.yml` records the upstream repository, the
exact commit and the build counter. Three ways a version is produced:

| Situation | Command | Version |
| --- | --- | --- |
| Next delivery (default) | `make vos-release` | `0.8.2-vos.2`, or `0.9.0-vos.1` once `mirror` has `v0.9.0` merged |
| Force the same core, without looking at `mirror` | `make vos-release ARGS="--same"` | `0.8.2-vos.2` |
| Move to a specific newer upstream release | `make vos-release ARGS="--upstream 0.9.0"` | `0.9.0-vos.1` |
| We raised the core ourselves | `make vos-release ARGS="--bump minor"` | `0.9.0-vos.1+up0.8.2` |

**The default follows what local `mirror` has already merged.** With no arguments the script takes
the core from the newest release tag reachable from local `mirror`, and increments the build when
`mirror` has not moved. A tag that was only fetched, or only exists upstream, is deliberately
ignored — that is the whole point of the rule: the order is sync first (`git fetch upstream --tags`;
`git switch mirror && git merge --ff-only v<new>`; `git switch main && git rebase mirror`), then
release. Reading it locally keeps a release working offline, and it is what makes the recorded base
a commit this branch really contains: the script refuses to record one it does not.

`UPSTREAM.yml` keeps two separate fields, because they answer different questions:

| Field | Meaning |
| --- | --- |
| `ref` | the upstream **release** commit (the `v<core>` tag) — provenance, cited in the image label |
| `base_sha` | the commit this fork **diverged** from (merge-base with `mirror`) — what `verify-scope.sh` diffs against |

They always coincide under the current scheme, because `mirror` is pinned to a release tag and the
fork branches off it. They are kept separate because using `ref` for the diff is only safe while
that holds: on a `mirror` that tracked upstream's tip instead, the tag would sit far behind the fork
point and every upstream commit since the tag would be reported as fork drift (352 files on 0.8.2).

The leading number is always the **real upstream core** and `-vos.<n>` is our own delivery counter;
`+up<base>` only appears when we deliberately raised the core above the upstream base (it records
that base, because the leading number no longer means "this is upstream X").

**Why `+up` is build metadata and not part of the leading number.** SemVer ignores everything after
`+` for both precedence and equality, which is exactly what we want: `0.9.0-vos.1+up0.8.2` keeps the
upstream *provenance* readable in `UPSTREAM.yml` / the OCI label without claiming upstream has a
0.9.0. One caveat follows from that, and it is handled: the store cannot order two builds that
differ only in their metadata, so each release still bumps `-vos.<n>`. The metadata never reaches a
Docker tag, which is a second reason the tag says nothing about the version at all.

**Image tags** are fixed `<arch>_<YYYYMMDD>`. Version and revision remain in OCI labels.
Set `PHOTOCRAFT_BUILD_SHA` from the local checkout and rsync it before invoking `docker/build_image.sh`
on the matching build host. Build hosts never run Git. Package validation also accepts prior SHA-suffixed
references so existing packages remain auditable and rollback-compatible.

**Cutting a release:** build and record both architectures with `docker/build_image.sh`, then
`make vos-release`, which commits `VERSION` + `UPSTREAM.yml`, tags `vos-photocraft-v<version>` and
pushes. `.github/workflows/vos-release.yml` verifies the tag against `VERSION`, runs the scope
guard, builds and audits the pull package, creates a draft GitHub Release (the fork is private) and
uploads to the App Store when `APPSTORE_IP`/`APPSTORE_TOKEN` are configured.

## Upstream sync

The branch layout is two branches, with distinct jobs:

| Branch | Job | How it moves |
| --- | --- | --- |
| `main` | the VOS work: what ships, and the repo's default branch | normal commits; `git rebase mirror` on a sync |
| `mirror` | a mirror of an upstream **release tag** (`v<core>`), never of upstream's tip | only `git merge --ff-only v<new>`; **never committed to** |

VOS changes are restricted to the paths in `ictrek.app/scripts/verify-scope.sh` (run it any time with
`make vos-scope`). After a new upstream release:

```bash
git fetch upstream --tags
git switch mirror && git merge --ff-only v<new upstream version>
git switch main && git rebase mirror
./ictrek.app/scripts/update_version.sh --upstream <new upstream version>
```

The sync rebase brings upstream's own `.github/workflows/*` back in (they are part of the
upstream tree). Three defences keep the fork's CI light:

1. **Prune commit**: the VOS series starts with a commit that deletes every upstream workflow.
   The rebase replays it and deletes whatever came back.
2. **rerere**: enable once with `git config rerere.enabled true`. When upstream *modifies* a
   workflow file, the prune commit hits a delete/modify conflict; resolve it once (keep the
   deletion) and rerere resolves it automatically on every later sync.
3. **CI gate**: `vos-ci.yml` has a "No foreign workflows" step that fails when any workflow other
   than `vos-release.yml`/`vos-ci.yml` exists — so a workflow rerere has not seen yet turns CI
   red instead of silently running upstream's heavy jobs.

Keeping the upstream mirror on a separate branch is what makes the sync a rebase instead of a merge:
`main` stays linear, so `mirror..main` is exactly "our changes" and the scope gate can diff them.
Pinning `mirror` to a tag rather than to `upstream/main` is deliberate: a release tag is a point
upstream has already validated end to end (its web/ wasm gate ran on it), while `upstream/main` is a
moving tip that can be broken between releases — `cargo xtask wasm` failed on it for 38 commits in
October 2026 (`crates/ui-egui/src/tile_overlay.rs` used `rayon` without the `wasm32` gate), which
broke every VOS web build until this branch was based on `v0.8.2` instead. Syncing tag-to-tag also
keeps `ictrek.app/VERSION`'s core honest: it names a version upstream actually published.

The upstream commit recorded in `UPSTREAM.yml` is resolved from the upstream release tag
`v<core>` (falling back to `upstream/v<core>` if you keep such a tag); `--ref <sha>` overrides both
when the base has no local tag.

## Phase 2: VOS storage (implemented)

The web build opens and saves documents in the platform's storage. Almost everything lives in
`apps/photocraft-web/src/vos/`: the plugin fills the shell's existing `Services` seams
(`file_dialog`, `write`) and draws its own window. Listing the user's documents on the Home
screen needs one small seam in `crates/ui-egui` (see "The Home screen" below), which is the
only core change this phase makes.

| File | Role |
| --- | --- |
| `vos.rs` | Module layout and the `cfg` gates (documented there) |
| `vos/path.rs` | The `<adapter>://<dir>/<name>` grammar, pure and host-tested |
| `vos/model.rs` | Finder reply decoding and the error-code wording, pure and host-tested |
| `vos/open.rs` | The `?open=` handoff: query parsing and same-origin validation, pure and host-tested |
| `vos/state.rs` | The picker's state machine, pure and host-tested |
| `vos/crypto.rs` | base64url + SHA-256 in Rust (the PKCE digest), pure and host-tested |
| `vos/bridge.rs` | `window.vos_platform` detection, PKCE, per-tab token cache |
| `vos/http.rs` | `fetch` wrappers for `index` / `download` / `upload` / the volume list |
| `vos/recent.rs` | The documents VOS holds, newest first, pure and host-tested |
| `vos/picker.rs` | The Finder window (egui) |
| `vos/home.rs` | The Home screen's VOS section: what to list, and the `localStorage` store |
| `vos/install.rs` | Wires the dialogs and the Home list into `Services`, drives the async flow |

**Identity.** `window.vos_platform.api.v1000.oauth2.authorize()` + `token()` (the public PKCE
client declared in `ictrek.app/src/manifest.yml`). The app's own access token carries the user's
permission codes, so Finder calls run **as the signed-in user** and the platform enforces RBAC;
the platform's JWT is never read. The token lives in `sessionStorage` for the tab.

Two things about that flow are worth keeping in mind:

- **The S256 challenge is computed in Rust** (`vos/crypto.rs`), not with `crypto.subtle`. `subtle`
  only exists in a secure context, so a box reached over plain `http://` (the usual first contact
  with a LAN VOS install) has `window.crypto` but no `crypto.subtle`, and the first sign-in used to
  fail with "Cannot read properties of undefined (reading 'digest')" before the platform was even
  asked. `crypto.getRandomValues`, which supplies the verifier, is not restricted that way.
- **The bridge's promises belong to the parent frame.** Its functions are created in the platform
  page and only injected into the iframe's `window`, so their promise fails an `instanceof Promise`
  test (what `dyn_into` uses). Every call goes through `bridge::call`, which adopts the thenable
  with `js_sys::Promise::resolve` and returns a promise of our own realm.

**Behaviour without VOS.** `install` checks for the bridge and does nothing when it is absent, so
a local `trunk serve` or a directly-opened page keeps the browser picker and the download.

**Saving.** A Finder path becomes the document's path, so Ctrl+S writes back to the same file
(`saves_in_place` accepts `.psd`/`.psb`/`.pcraft`/`.ora`). Uploads send
`conflict_strategy=overwrite`, because the platform's default rejects an existing name.

### Handoff from another app (`?open=`, `vos/open.rs`)

Another VOS app opens a file in PhotoCraft with
`/app/com.ictrek.photocraft/?open=<path>&name=<display name>&source=<app id>` (the contract is
`plan/image-studio-integration.md` §2; image-studio's gallery uses it). `vos/open.rs` parses and
validates it, `vos/install.rs` fetches it once the app exists, then:

- a **same-origin app path** (`/api/<app>/…` or `/app/<app>/…`) is fetched with the app's own
  access token and opened through `open_bytes`, so the source stays only a source: Ctrl+S goes to
  Save As (browser download, or the Finder picker), never back to the URL;
- a **Finder file path** (`v_…://…`) goes through the usual Finder download and `open_file`, so
  Ctrl+S writes back to it. A volume directory is refused.

Only root-relative paths are accepted, which makes them same-origin by construction, and the path
must sit in an app namespace (a platform endpoint such as `/api/v1/…` is refused). Full URLs,
other schemes, `..` (including percent-encoded and multiply-encoded), backslashes and control
characters are refused. Even so, the handoff performs an authenticated GET: a crafted link can
make the browser call any *app* endpoint in the user's name, so app APIs must keep GET free of
side effects. No token is ever put in the URL: PhotoCraft authenticates with its own
`window.vos_platform` token, and the source app's API accepts it because it validates the token's
user, not its audience. The first open in a fresh tab has no cached token (it lives in
`sessionStorage`), so it always runs the platform's sign-in fastpath. After the file opens, only
`open`, `name` and `source` are removed from the URL; other parameters such as `?cpu` and `?webgl`
survive (a failed handoff keeps everything, so a reload retries). The app whose id is in `source`
is not called back yet: saving a new version into image-studio's gallery is the next step, see
"Not yet built".

### Where a document lands, and the one row the picker needs

The app has no private document store. VOS saves write directly into the directory selected
from the application's data-access grants, so uninstalling or upgrading PhotoCraft preserves them.
Existing personal-volume Finder paths still map to `{mount_point}/users/{username}/data/...`.

The picker now reads the directories granted in **VOS App management → Data access authorization**.
The platform mounts `${VOS_APP_EXPOSED_PATH}` at `/exposed` with `rslave` propagation.
A small Rust storage service in the web image authenticates every request against the configured
`VOS_OIDC_USERINFO_URL`, then lists public and current-user `users/<username>/data` anchors.
Filesystem resolution rejects traversal and symlinks leaving an authorized root. Read-only mounts
remain read-only. Files are written atomically beside the destination, without an app-private copy.
No user-volume grant is required; admin follows the same authorization boundary as other apps.
Legacy Finder paths remain readable for existing recent documents and `?open=` links.

**File → Open** preserves the browser picker and adds separate VOS and AI Album choices. Command-specific
extension and multiple-selection options are preserved. Save offers this computer or VOS.
AI Album uses its existing `/api/oauth/fastpath` endpoint with an AI Album audience token obtained through its registered public PKCE client and the current VOS bridge session, then
queries image metadata/thumbnails and downloads the selected original using an in-memory album session.
There are no shared admin credentials, saved API keys or new album authentication bypasses.
An image opens as an editable copy, preserving the album original. Album access errors are shown with retry.

The VOS bridge is subscribed to `locale-change` before reading `getState()`. A revision guard prevents
an old snapshot from overriding a newer event. The subscription retries while the bridge is being
injected. Changes update `interface.language` and request the matching served fonts before the next UI frame.

### The Home screen

The Home screen (Photoshop's start screen) already exists and is on by default
(`general.autoShowHomeScreen`). On the web its recent list was **deliberately empty**:
`canvas.rs`'s `start_screen` skips `recent_files` under `cfg!(target_arch = "wasm32")`, because
a browser page has no local paths to reopen, and File > Open Recent answers "unavailable"
there. That left a refresh looking like a blank new app.

Two seams in `crates/ui-egui` close that gap. They name *platform storage*, not VOS, so they are
generic enough to carry upstream:

| Item | Role |
| --- | --- |
| `Services::platform_files` | Returns a `PlatformListing` (heading, an optional browse row, the files) for the Home screen. |
| `Services::platform_open` | Receives a `PlatformRequest` (`Open(id)`, `Browse`, or `Forget(id)` from a row's ×) when a row is clicked. |
| `PlatformFile` / `PlatformListing` / `PlatformRequest` | The data: an opaque `id` the shell hands straight back, a `name`, and a `detail` line. |
| `canvas.rs::home_platform` | Draws the section; it owns no state and does no I/O. |

The plugin fills both: `platform_files` reads `vos/recent.rs` (the paths it has opened or saved,
persisted in `localStorage` under `photocraft.vos.recent`), and `platform_open` turns a click
into a Finder download or into the ordinary File > Open. The Finder has no "recent" endpoint and
its entries carry no modification time, so the list has to be the plugin's own.

Asynchronous results cannot hold `&mut PhotocraftApp` across an `await`, so a downloaded
document waits in the plugin's state and `vos::ui` opens it on a later frame — the same shape as
`Services::inbox`. Opening a listed file passes its **full Finder path** to `open_file`, so
`Ctrl+S` writes back to VOS while the tab shows only the file name; the path is then removed from
File > Open Recent (the web cannot reopen one), leaving the Home list as the web's recent list.

A failed upload used to be silent: the plugin's `write` wrapper takes the bytes and returns `Ok`
so the engine marks the document clean, while the upload's own failure only reached the log. It
now reports through `app.ui.status`/`status_error` and `notices::error`.

Uploads of the same path are ordered: a save staged while an older upload of that file is still
out waits for it, so an older copy can never land on top of a newer one. A save that lands
during an upload cancels the failure report of the one before it (the newer copy is still coming).

A `10001` (expired session) is retried once with a fresh token by every Finder call, including
the download and the upload, which used to dead-end in "sign in again and retry".

### Why Finder and not the exposed-path grant (数据访问控制)

The platform also offers per-app directory grants, surfaced as the app store's 「数据访问控制」
dialog. They are the wrong tool here: a grant **bind-mounts** a directory into the app container
at `VOS_APP_EXPOSED_PATH`, and its HTTP API returns metadata only. This app is a static wasm page
in a browser — it has no container filesystem to read — so a grant would only be useful with a
backend container serving those bytes. A grant is also application-wide (nginx has no user
identity), which would expose one user's files to every other signed-in user. Finder is plain
same-origin HTTP with the platform doing per-user isolation, so it needs no backend at all.
An exposed-path-backed **shared media library** becomes interesting only once a BFF exists;
app-private data (autosave, thumbnails) would then go to `${VOS_APP_STORAGE_PATH}/users/<sub>/`.

### Not yet built

- **Saving a new version back into the app that handed the file over.** `?open=` opens it and
  Ctrl+S saves to VOS (or downloads), but nothing posts the result to image-studio's gallery
  (`POST /api/v1/outputs/import`, described in `plan/image-studio-integration.md` §4).
- **Autosave / crash recovery.** `Services::autosave`/`recover` are still unset on the web, so a
  refresh loses an edited document unless it was saved to VOS.
- **New Folder, rename, delete, shares, public directories.** The Finder API has them; the picker
  currently browses personal-space volumes only.
- **Large files.** Downloads are fetched whole into wasm memory; a very large PSD may need
  streaming or a BFF.
- **UI language.** The picker's strings are English (`i18n::t` falls back to the source string);
  they are deliberately not `tl!` literals, whose catalogs live in `crates/`.

### Upstream sync

This phase adds registered deviations. Most are new files inside the fork's `apps/` scope, so
they never conflict: `apps/photocraft-web/src/vos.rs` and `apps/photocraft-web/src/vos/*`.
Small edits: `apps/photocraft-web/src/main.rs` (one `mod`), `.../web.rs` (install + one draw
call), `.../Cargo.toml` (`serde`/`serde_json`, already in the workspace, plus four `web-sys`
features).

The Home screen additionally touches core, minimally and generically — it names platform storage,
not VOS:

- `crates/ui-egui/src/lib.rs`: two `Services` fields (`platform_files`, `platform_open`) and three
  public types (`PlatformFile`, `PlatformListing`, `PlatformRequest`).
- `crates/ui-egui/src/canvas.rs`: `start_screen` asks for the listing and draws the section
  (`home_platform`), with its test.

Both are additive: every other `Services` literal uses `..Default::default()`, and the desktop
path is unchanged (`platform_files` stays `None`, so the local recent list is used as before).
These are the two core files `verify-scope.sh` reports after committing — note it diffs
`base..HEAD`, so **uncommitted work is invisible to it**. Run `make vos-scope` after committing to
see the full list.

Tests: `cargo test -p photocraft-web` covers the pure modules (path grammar, reply decoding,
picker transitions, the recent list). `cargo test -p photocraft-ui-egui --lib canvas::tests`
covers the Home-screen seam (`the_home_screen_lists_platform_files_and_routes_their_clicks`).
The browser modules are compile-checked by `cargo xtask wasm`.


### Export finished images to AI Album

Save and Export retain browser download and VOS destinations. For PNG, JPEG and WebP output,
the destination chooser also offers **Export image to AI Album**. The same user session uploads
the encoded result, checks the returned asset through the album API, and only then confirms success.
Original album images are preserved. Layered PSD/PCraft projects stay on this computer or VOS.
Failed asynchronous saves report the failure and restore the document's unsaved state so it can be retried.
