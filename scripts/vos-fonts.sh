#!/usr/bin/env bash
# Copy the served fonts into a build's staging directory. Trunk runs this as a `post_build` hook
# (apps/photocraft-web/Trunk.vos-dev.toml) on every rebuild — trunk wipes and replaces its dist
# directory, so the fonts have to be re-added each time, exactly like the upstream font guide
# describes.
#
#   scripts/vos-fonts.sh [staging-dir]     # the hook passes no argument: trunk exports
#                                         # TRUNK_STAGING_DIR, and command_arguments are passed to
#                                         # the process without shell expansion, so $VAR would arrive
#                                         # literally.
#
# The destination is resolved through any symlinks first: trunk's staging directory is
# ../../dist/web/.stage, which is outside the app directory. A missing font folder is a warning, not
# an error, so the dev server still starts (without CJK coverage) when craft-fonts is not checked
# out. A path that *is* set but incomplete is an error: silently serving fewer fonts is worse.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${HERE}/.." && pwd)"

cleanup() { rm -f "${list:-}"; }
trap cleanup EXIT

dest="${1:-${TRUNK_STAGING_DIR:-}}"
if [ -z "$dest" ]; then
  echo "[fonts][ERROR] usage: vos-fonts.sh [staging-dir] (or set TRUNK_STAGING_DIR, which trunk exports for the post_build hook)" >&2
  exit 2
fi
if [ -d "$dest" ]; then
  dest="$(cd "$dest" && pwd -P)"
else
  dest="$(cd "$(dirname "$dest")" && pwd -P)/$(basename "$dest")"
fi

# Same sources scripts/vos-dev.sh uses: an explicit folder wins, then craft-fonts, then nothing.
if [ -n "${PHOTOCRAFT_WEB_FONTS_DIR:-}" ]; then
  src="$PHOTOCRAFT_WEB_FONTS_DIR"
elif [ -n "${CRAFT_FONTS_DIR:-}" ] && [ -f "${CRAFT_FONTS_DIR}/fonts/manifest.txt" ]; then
  src="$CRAFT_FONTS_DIR"
else
  echo "[fonts] no craft-fonts (set CRAFT_FONTS_DIR or PHOTOCRAFT_WEB_FONTS_DIR): serving no extra fonts"
  exit 0
fi

manifest="${src}/fonts/manifest.txt"
if [ ! -f "$manifest" ]; then
  echo "[fonts] ${manifest} is missing: serving no extra fonts"
  exit 0
fi

mkdir -p "${dest}/fonts"
cp "$manifest" "${dest}/fonts/manifest.txt"
# Field 3 is the font file and field 6 its licence file, both relative to the folder. The list is
# materialised first so that a missing file fails the build instead of silently serving less.
list="$(mktemp)"
awk -F' [|] ' '!/^[[:space:]]*(#|$)/ { print $3; if ($6 != "") print $6 }' "$manifest" | sort -u > "$list"
copied=0
while IFS= read -r f; do
  [ -n "$f" ] || continue
  case "$f" in
    /*|*..*|*\\*|*://*)
      echo "[fonts][ERROR] path outside the font folder: $f" >&2
      exit 1
      ;;
  esac
  if [ ! -f "${src}/${f}" ]; then
    echo "[fonts][ERROR] ${manifest} lists ${f}, which is not in ${src}" >&2
    exit 1
  fi
  mkdir -p "${dest}/$(dirname "$f")"
  cp "${src}/${f}" "${dest}/${f}"
  copied=$((copied + 1))
done < "$list"
rm -f "$list"

echo "[fonts] served ${copied} file(s) listed by ${manifest} -> ${dest}"
