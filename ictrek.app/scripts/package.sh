#!/usr/bin/env bash
# Build the VOS pull-mode package for com.ictrek.photocraft.
#
#   ./scripts/package.sh                       # read image tags from Feishu, render, tar, audit
#   ./scripts/package.sh --dry-run             # no Feishu: placeholder tags, for template checks
#   ./scripts/package.sh --audit-only PATH     # just audit an existing package
#
# The package is always pull mode: one outer tar holding only app.tar.gz. Image references live in
# the package's .env and are rendered into docker-compose.yml here; no image is embedded.
#
# Version: ictrek.app/VERSION, <upstream core>-vos.<n>[+up<base>], written by update_version.sh.
set -euo pipefail

APP_NAME="photocraft"
IMAGE_REPO="${PHOTOCRAFT_WEB_IMAGE_REPO:-swr.cn-southwest-2.myhuaweicloud.com/ictrek/photocraft-web}"
SPREADSHEET_TOKEN="${FEISHU_SPREADSHEET_TOKEN:-Htotsn3oahO1zxt73YMcaB1zn8e}"
FEISHU_CONFIG_FILE="${FEISHU_CONFIG_FILE:-${HOME}/.feishu.components.json}"
FEISHU_FALLBACK_CONFIG_FILE="${FEISHU_FALLBACK_CONFIG_FILE:-${HOME}/.feishu.json}"

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_DIR="$(cd "${HERE}/.." && pwd)"
SRC_DIR="${APP_DIR}/src"
DIST_DIR="${APP_DIR}/dist"
STAGE_DIR="${DIST_DIR}/staging"
PACKAGE_ROOT="${DIST_DIR}/package-root"
VERSION_FILE="${APP_DIR}/VERSION"
LOCK_DIR="${DIST_DIR}/.package.lock"

# The same image serves every platform: profiles only pick an architecture-specific base image, so
# any sheet of the right architecture may carry its tag. First non-empty wins.
ARCHES=(
  "amd|AMD_with_mxn100,AMD_with_cuda"
  "arm|ARM_without_cuda,ARM_with_cuda,l4t,thor_spark,SOPHON_bm1688"
)
COMPONENT="photocraft-web"
# Keep TAG_RE in sync with scripts/audit-package.sh.
TAG_RE='^(amd|arm)_[0-9]{8}_[0-9a-f]{7,40}(-r[0-9]+)?$'
VERSION_RE='^[0-9]+\.[0-9]+\.[0-9]+(-vos\.[0-9]+)?(\+[0-9A-Za-z.-]+)?$'

usage() {
  cat <<'EOF'
Usage:
  ./scripts/package.sh [--dry-run] [--version X.Y.Z-vos.N] [--audit-only PATH]

  --dry-run        do not read Feishu; write placeholder image tags (never publish such a package)
  --version V      package this version instead of ictrek.app/VERSION (CI passes the tag)
  --audit-only P   audit an already built package and exit

Output: dist/photocraft_<version>_pull.tar
EOF
}

log() { echo "[package] $*"; }
die() { echo "[package][ERROR] $*" >&2; exit 1; }

DRY_RUN=0
PACKAGE_VERSION=""
AUDIT_ONLY=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run) DRY_RUN=1; shift ;;
    --version) PACKAGE_VERSION="${2:-}"; shift 2 ;;
    --audit-only) AUDIT_ONLY="${2:-}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown argument: $1 (see --help)" ;;
  esac
done

if [[ -n "${PACKAGE_VERSION}" && ! "${PACKAGE_VERSION}" =~ ${VERSION_RE} ]]; then
  die "invalid --version: ${PACKAGE_VERSION}"
fi
if [[ -n "${AUDIT_ONLY}" ]]; then
  [[ -f "${AUDIT_ONLY}" ]] || die "no such package: ${AUDIT_ONLY}"
  exec "${HERE}/audit-package.sh" "${AUDIT_ONLY}"
fi

read_version() {
  [[ -f "${VERSION_FILE}" ]] || die "missing ${VERSION_FILE}"
  local value
  value="$(tr -d '[:space:]' < "${VERSION_FILE}")"
  [[ "${value}" =~ ${VERSION_RE} ]] || die "invalid VERSION: ${value}"
  printf '%s' "${value}"
}

for cmd in curl python3 tar; do
  command -v "${cmd}" >/dev/null || die "missing ${cmd}"
done

acquire_lock() {
  # A second concurrent run would clobber DIST_DIR mid-tar; fail fast rather than wait.
  mkdir "${LOCK_DIR}" 2>/dev/null || die "another package.sh is running (${LOCK_DIR}); remove it if that is wrong"
  trap 'rm -rf "${LOCK_DIR}"' EXIT
}

# ---------------------------------------------------------------- Feishu (image tag lookup)

read_feishu_field() {
  python3 - "$1" "$2" <<'PY'
import json, sys
path, field = sys.argv[1], sys.argv[2]
with open(path, "r", encoding="utf-8") as handle:
    data = json.load(handle)
value = data.get(field, "")
print(value if isinstance(value, str) else str(value))
PY
}

feishu_api() {
  local method="$1" url="$2" token="$3"
  curl --fail -sS -X "${method}" "${url}" -H "Authorization: Bearer ${token}"
}

get_feishu_token() {
  local app_id="$1" app_secret="$2" response
  response="$(curl --fail -sS -X POST \
    'https://open.feishu.cn/open-apis/auth/v3/tenant_access_token/internal' \
    -H 'Content-Type: application/json' \
    -d "{\"app_id\":\"${app_id}\",\"app_secret\":\"${app_secret}\"}")"
  python3 - "${response}" <<'PY'
import json, sys
data = json.loads(sys.argv[1])
if data.get("code") != 0:
    raise SystemExit("get_feishu_token failed: " + str(data))
print(data["tenant_access_token"])
PY
}

load_feishu_auth() {
  local config_file tried=""
  for config_file in "${FEISHU_CONFIG_FILE}" "${FEISHU_FALLBACK_CONFIG_FILE}"; do
    [[ -n "${config_file}" && "${tried}" != *"|${config_file}|"* ]] || continue
    tried="${tried}|${config_file}|"
    [[ -r "${config_file}" ]] || { log "skip unreadable Feishu config: ${config_file}"; continue; }
    if FEISHU_APP_ID="$(read_feishu_field "${config_file}" feishu_app_id)" \
      && FEISHU_APP_SECRET="$(read_feishu_field "${config_file}" feishu_app_secret)" \
      && [[ -n "${FEISHU_APP_ID}" && -n "${FEISHU_APP_SECRET}" ]] \
      && FEISHU_TOKEN="$(get_feishu_token "${FEISHU_APP_ID}" "${FEISHU_APP_SECRET}")"; then
      log "reading image tags from Feishu with ${config_file}"
      return 0
    fi
    log "cannot read Feishu auth from ${config_file}; trying fallback"
  done
  die "failed to read Feishu credentials (or use --dry-run)"
}

get_sheet_id_by_title() {
  local token="$1" title="$2" response
  response="$(feishu_api GET \
    "https://open.feishu.cn/open-apis/sheets/v3/spreadsheets/${SPREADSHEET_TOKEN}/sheets/query" "${token}")"
  python3 - "${title}" "${response}" <<'PY'
import json, sys
title, response = sys.argv[1], sys.argv[2]
data = json.loads(response)
if data.get("code") != 0:
    raise SystemExit("query sheets failed: " + str(data))
for sheet in data.get("data", {}).get("sheets", []):
    if sheet.get("title") == title:
        print(sheet["sheet_id"])
        raise SystemExit(0)
raise SystemExit("sheet title not found: " + title)
PY
}

# Row 1 header cells as a "|"-joined string (rich text cells come back as nested structures).
header_row() {
  python3 - "$1" <<'PY'
import json, sys
data = json.loads(sys.argv[1])
if data.get("code") != 0:
    raise SystemExit("read header failed: " + str(data))


def text(value):
    if value is None:
        return ""
    if isinstance(value, str):
        return value.strip()
    if isinstance(value, dict):
        return str(value.get("text") or value.get("link") or "").strip()
    if isinstance(value, list):
        return "".join(text(item) for item in value).strip()
    return str(value).strip()


rows = data.get("data", {}).get("valueRange", {}).get("values", [])
print("|".join(text(cell) for cell in (rows[0] if rows else [])))
PY
}

# Prints "<repository>:<tag>" for one architecture, or nothing when no sheet carries an entry yet.
latest_image() {
  local token="$1" sheets="$2" sheet_title sheet_id header column repo response tag
  local candidates=()
  IFS=',' read -ra candidates <<< "${sheets}"
  for sheet_title in "${candidates[@]}"; do
    sheet_id="$(get_sheet_id_by_title "${token}" "${sheet_title}" 2>/dev/null)" || continue
    response="$(feishu_api GET \
      "https://open.feishu.cn/open-apis/sheets/v2/spreadsheets/${SPREADSHEET_TOKEN}/values/${sheet_id}!A1:ZZ2" "${token}")" || continue
    header="$(header_row "${response}")" || continue
    column="$(python3 - "${COMPONENT}" "${header}" <<'PY'
import sys
target, header = sys.argv[1], sys.argv[2]
for index, name in enumerate(header.split("|"), start=1):
    if name == target:
        print(index)
        raise SystemExit(0)
raise SystemExit(1)
PY
)" || continue
    tag="$(feishu_api GET \
      "https://open.feishu.cn/open-apis/sheets/v2/spreadsheets/${SPREADSHEET_TOKEN}/values/${sheet_id}!A4:ZZ2000" "${token}" \
      | python3 -c 'import json, sys
column = int(sys.argv[1])
rows = json.load(sys.stdin).get("data", {}).get("valueRange", {}).get("values", [])
for row in rows:
    if len(row) >= column and row[column - 1] is not None and str(row[column - 1]).strip():
        print(str(row[column - 1]).strip())
        break
else:
    raise SystemExit(1)' "${column}")" || continue
    if [[ ! "${tag}" =~ ${TAG_RE} ]]; then
      die "${sheet_title}: ${COMPONENT} tag ${tag} is not <arch>_<YYYYMMDD>_<sha>"
    fi
    printf '%s:%s\n' "${IMAGE_REPO}" "${tag}"
    return 0
  done
  return 1
}

# ------------------------------------------------------------------------------- render + pack

render_text_file() {
  local src="$1" dst="$2"
  python3 - "${src}" "${dst}" "${APP_VERSION}" <<'PY'
import sys
from pathlib import Path
src, dst, version = Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3]
dst.write_text(src.read_text(encoding="utf-8").replace("__APP_VERSION__", version), encoding="utf-8")
PY
}

render_compose_file() {
  local src="$1" dst="$2" env_file="$3"
  python3 - "${src}" "${dst}" "${APP_VERSION}" "${env_file}" <<'PY'
import re
import sys
from pathlib import Path

src, dst, version, env_path = Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3], Path(sys.argv[4])
env = {}
for line in env_path.read_text(encoding="utf-8").splitlines():
    if line and not line.startswith("#") and "=" in line:
        key, value = line.split("=", 1)
        env[key] = value

text = src.read_text(encoding="utf-8").replace("__APP_VERSION__", version)
text = re.sub(r"\$\{([A-Z0-9_]+)(?::-[^}]*)?\}", lambda m: env.get(m.group(1), m.group(0)), text)
dst.write_text(text, encoding="utf-8")
PY
}

validate_yaml() {
  python3 - "$1" <<'PY'
import sys
import yaml
if sys.version_info < (3, 10):
    raise SystemExit("python 3.10+ is required")
with open(sys.argv[1], "r", encoding="utf-8") as handle:
    yaml.safe_load(handle)
PY
}

APP_VERSION="${PACKAGE_VERSION:-$(read_version)}"
[[ "${APP_VERSION}" =~ ${VERSION_RE} ]] || die "invalid version: ${APP_VERSION}"
log "package version: ${APP_VERSION}"

mkdir -p "${DIST_DIR}"
acquire_lock
rm -rf "${STAGE_DIR}" "${PACKAGE_ROOT}"
mkdir -p "${STAGE_DIR}" "${PACKAGE_ROOT}"

ENV_FILE="${STAGE_DIR}/.env"
: > "${ENV_FILE}"

if (( DRY_RUN )); then
  log "dry run: image tags are placeholders"
  printf 'PHOTOCRAFT_WEB_AMD_IMAGE=%s:%s\n' "${IMAGE_REPO}" "DRY_RUN_DO_NOT_INSTALL" >> "${ENV_FILE}"
  printf 'PHOTOCRAFT_WEB_ARM_IMAGE=%s:%s\n' "${IMAGE_REPO}" "DRY_RUN_DO_NOT_INSTALL" >> "${ENV_FILE}"
else
  load_feishu_auth
  for spec in "${ARCHES[@]}"; do
    arch="${spec%%|*}"
    sheets="${spec#*|}"
    image="$(latest_image "${FEISHU_TOKEN}" "${sheets}")" || die "no ${COMPONENT} image registered for ${arch} (build and record one first)"
    log "${arch} -> ${image}"
    printf 'PHOTOCRAFT_WEB_%s_IMAGE=%s\n' "$(printf '%s' "${arch}" | tr '[:lower:]' '[:upper:]')" "${image}" >> "${ENV_FILE}"
  done
fi

for file in manifest.yml routers.yml; do
  [[ -f "${SRC_DIR}/${file}" ]] || die "missing ${SRC_DIR}/${file}"
  render_text_file "${SRC_DIR}/${file}" "${STAGE_DIR}/${file}"
  validate_yaml "${STAGE_DIR}/${file}"
done
render_compose_file "${SRC_DIR}/docker-compose.yml" "${STAGE_DIR}/docker-compose.yml" "${ENV_FILE}"
validate_yaml "${STAGE_DIR}/docker-compose.yml"
render_text_file "${SRC_DIR}/LICENSE-NOTICE.md" "${STAGE_DIR}/LICENSE-NOTICE.md"
[[ -f "${SRC_DIR}/icon.png" ]] || die "missing ${SRC_DIR}/icon.png"
cp "${SRC_DIR}/icon.png" "${STAGE_DIR}/icon.png"
render_text_file "${SRC_DIR}/README.zh-CN.md" "${STAGE_DIR}/README.zh-CN.md"
[[ -f "${SRC_DIR}/README.en.md" ]] && render_text_file "${SRC_DIR}/README.en.md" "${STAGE_DIR}/README.en.md"

APP_TARBALL="${DIST_DIR}/app.tar.gz"
PACKAGE_PATH="${DIST_DIR}/${APP_NAME}_${APP_VERSION}_pull.tar"
TAR_FILES=(.env manifest.yml docker-compose.yml routers.yml LICENSE-NOTICE.md icon.png README.zh-CN.md)
[[ -f "${STAGE_DIR}/README.en.md" ]] && TAR_FILES+=(README.en.md)
tar czf "${APP_TARBALL}" -C "${STAGE_DIR}" "${TAR_FILES[@]}"
cp "${APP_TARBALL}" "${PACKAGE_ROOT}/app.tar.gz"
tar cf "${PACKAGE_PATH}" -C "${PACKAGE_ROOT}" app.tar.gz

if (( DRY_RUN )); then
  "${HERE}/audit-package.sh" --allow-dry-run "${PACKAGE_PATH}"
  log "dry-run package built: ${PACKAGE_PATH} (placeholder images; never publish it)"
else
  "${HERE}/audit-package.sh" "${PACKAGE_PATH}"
  log "done: ${PACKAGE_PATH}"
fi
log "install locally: vos-platform-cli app install-local --package-path ${PACKAGE_PATH}"
