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
| `wasm-opt` / binaryen (114 MB) | **not needed** — trunk skips it outside a release build | inside the image, installed at toolchain-build time through the accelerator |

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

**Image tags** follow the VAI convention (`../vai/apps/nowen-video/build_image.sh`): one tag,
`<arch>_<YYYYMMDD>_<short sha>` (optionally `-r<N>` for a second build of the same commit the same
day), which is what Feishu records and both `package.sh` and `audit-package.sh` read (they validate
it against the same `TAG_RE`).

The version is deliberately not in the tag. It lives in the `org.opencontainers.image.version`
label, in `ictrek.app/VERSION` and in `ictrek.app/UPSTREAM.yml`, and the upstream base additionally
in the `com.ictrek.photocraft.upstream` label — which is also why the metadata above needs no tag of
its own.

A second `<version>-<arch>` tag used to be pushed as a "human-readable" alias. It was dropped: no
script referenced it, `TAG_RE` rejects that form, and since it did not carry `--tag-suffix` a second
build of the same day overwrote the first, so the by-version tag could not be pulled for both. Pull
the recorded tag instead:

```bash
docker pull swr.cn-southwest-2.myhuaweicloud.com/ictrek/photocraft-web:amd_20261014_5923bed5d
```

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
