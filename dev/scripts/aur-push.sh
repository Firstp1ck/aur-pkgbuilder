#!/usr/bin/env bash
# aur-push.sh — Push AUR package updates from the current PKGBUILD directory.
#
# What: runs `makepkg --nobuild`, refreshes `.SRCINFO`, commits, and pushes.
# For `-bin` packages, retries after `updpkgsums` when checksums drift.
#
# Usage:
#   cd <aur-clone>           # e.g. ssh://aur@aur.archlinux.org/aur-pkgbuilder-bin.git
#   ../aur-pkgbuilder/dev/scripts/aur-push.sh "commit message…"

set -euo pipefail

msg="${*:-Update package}"
OK="✅"
FAIL="❌"

if [[ ! -f PKGBUILD ]]; then
  printf "%s PKGBUILD not found in current directory: %s\n" "${FAIL}" "$(pwd)" >&2
  exit 1
fi

pkgname="$(awk -F= '/^pkgname=/{print $2; exit}' PKGBUILD | sed "s/[\"']//g")"
is_bin=0
if [[ "${pkgname}" =~ -bin$ ]]; then
  is_bin=1
fi

if makepkg --nobuild; then
  echo "${OK} makepkg --nobuild completed"
else
  if [[ "${is_bin}" -eq 1 ]]; then
    echo "ℹ️  makepkg --nobuild failed; attempting checksum refresh via updpkgsums for -bin package"
    if command -v updpkgsums >/dev/null 2>&1; then
      if updpkgsums; then
        echo "${OK} Checksums updated via updpkgsums"
        if makepkg --nobuild; then
          echo "${OK} makepkg --nobuild completed after checksum update"
        else
          echo "${FAIL} makepkg --nobuild still failing after checksum update" >&2
          exit 1
        fi
      else
        echo "${FAIL} updpkgsums failed" >&2
        exit 1
      fi
    else
      echo "${FAIL} updpkgsums not found (install pacman-contrib)" >&2
      exit 127
    fi
  else
    echo "${FAIL} makepkg --nobuild failed" >&2
    exit 1
  fi
fi

if makepkg --printsrcinfo > .SRCINFO; then
  echo "${OK} .SRCINFO updated"
else
  echo "${FAIL} .SRCINFO update failed" >&2
  exit 1
fi

if git add .; then
  echo "${OK} Staged PKGBUILD / .SRCINFO changes"
else
  echo "${FAIL} Failed to stage changes" >&2
  exit 1
fi

if git diff --quiet --cached; then
  echo "${OK} Nothing to commit (no staged changes)"
else
  if git commit -m "${msg}"; then
    echo "${OK} Committed: ${msg}"
  else
    echo "${FAIL} Commit failed" >&2
    exit 1
  fi
fi

# AUR's default branch is `master`.
if git push origin master; then
  echo "${OK} Pushed to origin master"
else
  echo "${FAIL} Push failed" >&2
  exit 1
fi
