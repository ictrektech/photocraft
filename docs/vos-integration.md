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
`wasm-bindgen-cli` comes from crates.io instead of a GitHub release download, so it builds on
networks where that download is blocked. It carries OCI labels naming the upstream base commit.

## Local development inside the platform

```bash
make vos-dev                 # trunk dev server on 38092 + a temporary Traefik route
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
so both modes serve the same URLs the shipped nginx configuration expects.

**The dev server must not listen on loopback.** Traefik reaches this workstation through the docker
bridge (`VOS_HOST_GW_IP`, `172.17.0.1`), so a `127.0.0.1`-only listener is unreachable from it: the
host sees a healthy port while the platform answers `502 Bad Gateway`. The script therefore binds
`VOS_DEV_BIND_ADDR` (default `0.0.0.0`, matching the sibling VOS apps' `vite --host`);
`VOS_DEV_BIND_ADDR=172.17.0.1` restricts it to the bridge. Both the readiness probe and
`make vos-dev-status` (`from traefik: reachable`) check the URL the platform actually requests, and a
failing check prints the fix instead of leaving you with a blank 502.

The script never touches the installed app's containers (the platform reconciles them every 60
seconds); it only adds and removes `/etc/traefik/dynamic-conf/com.ictrek.photocraft.dev.yml`.

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

**You need the wasm target.** Trunk mode compiles on this workstation, so the toolchain needs it
once (the build host and the Docker image have their own):

```bash
rustup target add wasm32-unknown-unknown
```

The tools, and which mode needs which:

| Tool | dev (`make vos-dev`) | fidelity (`VOS_DEV_WEB=0`) |
| --- | --- | --- |
| `wasm32-unknown-unknown` target | needed | needs only Docker |
| `wasm-bindgen` (0.2.129) | needed; `cargo install wasm-bindgen-cli --version 0.2.129 --locked` avoids trunk's own 8 MB download | inside the image |
| `wasm-opt` / binaryen (114 MB) | **not needed** — trunk skips it outside a release build | inside the image (trunk downloads it there) |

Fidelity mode needs no host Rust toolchain at all: `make vos-dev-web` builds `dist/web` through the
image's build stage, which needs only Docker — and it warns first if `wasm-opt` is missing, with the
resumable download commands for a slow link.

## Building the image and the package

```bash
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
| Next delivery (default) | `make vos-release` | `0.8.2-vos.2`, or `0.9.0-vos.1` once `main` has `v0.9.0` merged |
| Force the same core, without looking at `main` | `make vos-release ARGS="--same"` | `0.8.2-vos.2` |
| Move to a specific newer upstream release | `make vos-release ARGS="--upstream 0.9.0"` | `0.9.0-vos.1` |
| We raised the core ourselves | `make vos-release ARGS="--bump minor"` | `0.9.0-vos.1+up0.8.2` |

**The default follows what local `main` has already merged.** With no arguments the script takes the
core from the newest release tag reachable from local `main`, and increments the build when `main`
has not moved. A tag that was only fetched, or only exists upstream, is deliberately ignored — that
is the whole point of the rule: the order is sync first (`git fetch upstream --tags`;
`git switch main && git merge --ff-only v<new>`; `git rebase main`), then release. Reading it
locally keeps a release working offline, and it is what makes the recorded base a commit this branch
really contains: the script refuses to record one it does not.

`UPSTREAM.yml` keeps two separate fields, because they answer different questions:

| Field | Meaning |
| --- | --- |
| `ref` | the upstream **release** commit (the `v<core>` tag) — provenance, cited in the image label |
| `base_sha` | the commit this fork **diverged** from (merge-base with `main`) — what `verify-scope.sh` diffs against |

They always coincide under the current scheme, because `main` is pinned to a release tag and the fork
branches off it. They are kept separate because using `ref` for the diff is only safe while that
holds: on a `main` that tracked upstream's tip instead, the tag would sit far behind the fork point
and every upstream commit since the tag would be reported as fork drift (352 files on 0.8.2).

The leading number is always the **real upstream core** and `-vos.<n>` is our own delivery counter;
`+up<base>` only appears when we deliberately raised the core above the upstream base (it records
that base, because the leading number no longer means "this is upstream X").

**Why `+up` is build metadata and not part of the leading number.** SemVer ignores everything after
`+` for both precedence and equality, which is exactly what we want: `0.9.0-vos.1+up0.8.2` keeps the
upstream *provenance* readable in `UPSTREAM.yml` / the OCI label without claiming upstream has a
0.9.0. Two caveats follow from that, and both are handled: the store cannot order two builds that
differ only in their metadata (so each release still bumps `-vos.<n>`), and Docker tags allow
`[A-Za-z0-9_.-]` only (so `docker/build_image.sh` strips the metadata for the tag and keeps the full
version in the image labels).

**Image tags** follow the VAI convention (`../vai/apps/nowen-video/build_image.sh`): the main tag
`<arch>_<YYYYMMDD>_<short sha>` (optionally `-r<N>` for a second build of the same commit the same
day) is what Feishu records and `package.sh` reads; the release tag `<version>-<amd64|arm64>` is the
human-readable one. The upstream base is in neither tag — it lives in the image labels
(`com.ictrek.photocraft.upstream`) and in `UPSTREAM.yml`.

**Cutting a release:** build and record both architectures with `docker/build_image.sh`, then
`make vos-release`, which commits `VERSION` + `UPSTREAM.yml`, tags `vos-photocraft-v<version>` and
pushes. `.github/workflows/vos-release.yml` verifies the tag against `VERSION`, runs the scope
guard, builds and audits the pull package, creates a draft GitHub Release (the fork is private) and
uploads to the App Store when `APPSTORE_IP`/`APPSTORE_TOKEN` are configured.

## Upstream sync

`main` is pinned to an upstream **release tag** (`v<core>`), never to upstream's tip; VOS work lives
on `vos/` and is restricted to the paths in `ictrek.app/scripts/verify-scope.sh` (run it any time
with `make vos-scope`). After a new upstream release:

```bash
git fetch upstream --tags
git switch main && git merge --ff-only v<new upstream version>
git switch vos && git rebase main
./ictrek.app/scripts/update_version.sh --upstream <new upstream version>
```

Pinning `main` to a tag rather than to `upstream/main` is deliberate: a release tag is a point
upstream has already validated end to end (its web/ wasm gate ran on it), while `upstream/main` is a
moving tip that can be broken between releases — `cargo xtask wasm` failed on it for 38 commits in
October 2026 (`crates/ui-egui/src/tile_overlay.rs` used `rayon` without the `wasm32` gate), which
broke every VOS web build until this branch was based on `v0.8.2` instead. Syncing tag-to-tag also
keeps `ictrek.app/VERSION`'s core honest: it names a version upstream actually published.

The upstream commit recorded in `UPSTREAM.yml` is resolved from the upstream release tag
`v<core>` (falling back to `upstream/v<core>` if you keep such a tag); `--ref <sha>` overrides both
when the base has no local tag.

## Phase 2 (designed, not implemented)

The manifest already declares the platform OAuth2 client and the compose file reserves the place
for a thin BFF, so this can be added without touching the delivery shape:

- **Files**: open and save documents from VOS Finder (`/v1000/finder_next/{index,download,save,upload}`)
  instead of the browser picker. This is the only change that reaches into `crates/ui-egui`
  (the `Services` file-dialog seam) and it is deliberately deferred.
- **Identity**: `window.vos_platform.api.v1000.oauth2.*` (OIDC fastpath) for the current user; the
  web shell would also pick up the platform's UI language.
- **Storage**: nothing is persisted by the app itself; if that ever changes, per-user data goes to
  `${VOS_APP_STORAGE_PATH}/users/<sub>/`.
