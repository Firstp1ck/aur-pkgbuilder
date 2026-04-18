#!/usr/bin/env bash
# update-sha256sums.sh — Refresh sha256sums in a PKGBUILD via `updpkgsums`.
#
# Usage:
#   # From within a directory containing a PKGBUILD:
#   ../aur-pkgbuilder/dev/scripts/update-sha256sums.sh
#
#   # Or pass a target directory explicitly:
#   dev/scripts/update-sha256sums.sh ./path/to/pkgbuild-dir

set -euo pipefail

OK="✅"
FAIL="❌"

target_dir="${1:-.}"

if [[ ! -f "$target_dir/PKGBUILD" ]]; then
  printf "%s PKGBUILD not found in: %s\n" "$FAIL" "$target_dir" >&2
  exit 1
fi

if ! command -v updpkgsums >/dev/null 2>&1; then
  printf "%s updpkgsums not found (install pacman-contrib)\n" "$FAIL" >&2
  exit 127
fi

pushd "$target_dir" >/dev/null

# Preserve the pre-update PKGBUILD so a bad download leaves a breadcrumb.
cp -f PKGBUILD PKGBUILD.bak

if updpkgsums; then
  rm -f PKGBUILD.bak
  printf "%s sha256sums refreshed in %s/PKGBUILD\n" "$OK" "$(pwd)"
else
  rc=$?
  # Roll the original file back so the directory is in a known-good state.
  mv -f PKGBUILD.bak PKGBUILD
  printf "%s updpkgsums failed (exit %d); PKGBUILD restored\n" "$FAIL" "$rc" >&2
  exit "$rc"
fi

popd >/dev/null
