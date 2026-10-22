#!/usr/bin/env bash
# Independent audit of a built VOS pull package: verify the exact tarball the platform would take.
#
#   ./scripts/audit-package.sh ictrek.app/dist/photocraft_0.8.2-vos.1_pull.tar
#
# Exits non-zero on the first structural problem, with a message naming it. package.sh runs it
# after building, and .github/workflows/vos-release.yml runs it on the artifact it is about to
# publish, so a broken package can never reach the App Store or a GitHub Release.
set -euo pipefail

APP_ID="com.ictrek.photocraft"
ROUTER_PAGE_ID="photocraft"
# The Feishu sheet convention for image tags (VAI-wide), optionally with a repeat-build suffix.
TAG_RE='^(amd|arm)_[0-9]{8}(_[0-9a-f]{7,40}(-r[0-9]+)?)?$'
VERSION_RE='^[0-9]+\.[0-9]+\.[0-9]+(-vos\.[0-9]+)?(\+[0-9A-Za-z.-]+)?$'

die() { echo "[audit][ERROR] $*" >&2; exit 1; }
log() { echo "[audit] $*"; }

allow_dry_run=0
package_path=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --allow-dry-run) allow_dry_run=1; shift ;;
    -h|--help) echo "usage: audit-package.sh [--allow-dry-run] <pull.tar>"; exit 0 ;;
    *) package_path="$1"; shift ;;
  esac
done
[[ -n "${package_path}" ]] || die "usage: audit-package.sh [--allow-dry-run] <pull.tar>"
[[ -f "${package_path}" ]] || die "no such package: ${package_path}"
command -v python3 >/dev/null || die "missing python3"

work="$(mktemp -d)"
trap 'rm -rf "${work}"' EXIT

log "reading ${package_path}"
tar tf "${package_path}" > "${work}/outer.list" 2>/dev/null || die "not a readable tar archive"

if grep -qE '^(\./)?assets/' "${work}/outer.list"; then
  die "pull mode must not embed assets/: images are pulled from the registry at install time"
fi
if grep -qvE '^(\./)?app\.tar\.gz$' "${work}/outer.list"; then
  die "the pull package may only contain app.tar.gz (found: $(grep -vE '^(\./)?app\.tar\.gz$' "${work}/outer.list" | head -3 | tr '\n' ' '))"
fi
grep -qE '^(\./)?app\.tar\.gz$' "${work}/outer.list" || die "app.tar.gz is missing from the outer archive"

tar xf "${package_path}" -C "${work}" >/dev/null 2>&1 || die "cannot extract the outer archive"
[[ -f "${work}/app.tar.gz" ]] || die "app.tar.gz did not extract"

python3 - "${work}/app.tar.gz" "${APP_ID}" "${ROUTER_PAGE_ID}" "${TAG_RE}" "${VERSION_RE}" "${allow_dry_run}" <<'PYAUDIT'
import re
import sys
import tarfile

tarball, app_id, page_id, tag_re, version_re, allow_dry_run = sys.argv[1:7]
problems = []


def bad(message):
    problems.append(message)


with tarfile.open(tarball, "r:gz") as tar:
    # tar writes "./name"; strip exactly that prefix (lstrip would also eat a leading dot).
    members = {re.sub(r"^\./", "", name) for name in tar.getnames()}
    required = ["manifest.yml", "docker-compose.yml", ".env", "routers.yml", "README.zh-CN.md", "icon.png"]
    for name in required:
        if name not in members:
            bad("app.tar.gz is missing " + name)

    def read(name):
        if name not in members:
            return ""
        handle = tar.extractfile(name)
        return handle.read().decode("utf-8", "replace") if handle else ""

    texts = {name: read(name) for name in required if name.endswith((".yml", ".md", ".env"))}
    for name, text in texts.items():
        leftover = sorted(set(re.findall(r"__[A-Z0-9_]+__", text)))
        if leftover:
            bad(name + " still has unrendered placeholders: " + ", ".join(leftover[:3]))

    try:
        import yaml
    except ImportError:
        yaml = None

    if yaml is None:
        print("[audit] note: PyYAML missing, only regex checks ran")
    else:
        manifest = yaml.safe_load(texts["manifest.yml"])
        compose = yaml.safe_load(texts["docker-compose.yml"])
        routers = yaml.safe_load(texts["routers.yml"])

        if manifest.get("id") != app_id:
            bad("manifest.id is " + repr(manifest.get("id")) + ", expected " + app_id)
        version = str(manifest.get("version", ""))
        if not re.match(version_re, version):
            bad("manifest.version " + repr(version) + " is not <core>-vos.<n>[+up<base>]")

        frontend = manifest.get("frontend") or {}
        if frontend.get("enabled") is not True or frontend.get("basePath") != "/app/" + app_id:
            bad("manifest.frontend must be enabled with basePath /app/" + app_id)

        profiles = {p.get("name") for p in manifest.get("profiles") or []}
        if profiles != {"amd", "arm"}:
            bad("manifest.profiles must be exactly amd and arm, found " + repr(sorted(profiles)))
        if manifest.get("require-profiles") != ["amd", "arm"]:
            bad("manifest.require-profiles must be [amd, arm]: an install that picks no profile starts no service")

        oauth = (manifest.get("oauth2") or {}).get("client") or {}
        if oauth.get("id") != app_id:
            bad("manifest.oauth2.client.id must be the app id (phase 2 signs in through it)")

        compose_profiles = set()
        for service in (compose.get("services") or {}).values():
            compose_profiles.update(service.get("profiles") or [])
        if not profiles <= compose_profiles:
            bad("compose has no services for profiles " + repr(sorted(profiles - compose_profiles)))

        if routers.get("router-type") != "flatten":
            bad("routers.yml must declare router-type: flatten for a single-entry app")
        pages = routers.get("routers") or []
        if len(pages) != 1 or pages[0].get("kind") != "page":
            bad("routers.yml must be exactly one top-level page")
        else:
            page = pages[0]
            if page.get("id") != page_id:
                bad("routers.yml page id must be " + page_id)
            expected = "/app/" + app_id + "/?v=" + version
            if page.get("iframe-src") != expected:
                bad("routers.yml iframe-src must be " + expected + ", found " + repr(page.get("iframe-src")))
            for flag in ("entry-point", "embed"):
                if page.get(flag) is not True:
                    bad("routers.yml page must set " + flag + ": true")

        for name, service in (compose.get("services") or {}).items():
            image = service.get("image", "")
            if not image or "$" + "{" in image:
                bad("service " + name + " still has an unrendered image reference: " + repr(image))
            elif "/" not in image or "." not in image.split("/")[0]:
                bad("service " + name + " must use a full registry reference: " + repr(image))

    env = dict(
        line.split("=", 1)
        for line in texts[".env"].splitlines()
        if line and not line.startswith("#") and "=" in line
    )
    tags = {}
    for arch in ("amd", "arm"):
        key = "PHOTOCRAFT_WEB_" + arch.upper() + "_IMAGE"
        image = env.get(key, "")
        if not image:
            bad(".env is missing " + key)
            continue
        if "/" not in image or "." not in image.split("/")[0]:
            bad(key + " must be a full registry reference: " + repr(image))
            continue
        tag = image.rsplit(":", 1)[-1]
        tags[arch] = tag
        if tag == "DRY_RUN_DO_NOT_INSTALL":
            if allow_dry_run == "1":
                print("[audit] note: " + key + " holds the dry-run placeholder: this package must never be published")
            else:
                bad("this is a --dry-run package and must never be published")
        elif not re.match(tag_re, tag):
            bad(key + " tag " + repr(tag) + " must be <arch>_<YYYYMMDD>_<sha> (the Feishu convention)")

    if len(tags) == 2 and tags["amd"] != tags["arm"]:
        print("[audit] note: amd and arm image tags differ: " + tags["amd"] + " vs " + tags["arm"])

for problem in problems:
    print("[audit][ERROR] " + problem, file=sys.stderr)
if problems:
    sys.exit(1)
print("[audit] package is consistent")
PYAUDIT
