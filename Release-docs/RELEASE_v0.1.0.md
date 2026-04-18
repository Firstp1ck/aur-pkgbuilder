# Release v0.1.0

First tagged release of **aur-pkgbuilder**: a GTK4 / libadwaita desktop app for AUR maintainers to sync PKGBUILDs, bump versions, validate, build, and publish from one place.

## Highlights

- **End-to-end workflow:** Home, connection checks, package sync, version step, validation, `makepkg` build, and AUR Git publish—with streaming logs in the app.
- **PKGBUILD editing:** In-app editor with quick fields, diff-friendly updates, and safer handling around `updpkgsums` when checksums already match.
- **SSH & AUR:** Guided AUR SSH key setup, `known_hosts` fingerprint surfacing, preflight probes, and publish gating when SSH is not verified.
- **Packages & paths:** JSONC config and registry, optional per-package `sync_subdir`, validated destinations, and folder pickers rooted on the window.
- **Shell & UX:** Tabbed main shell with workflow navigation, connection/validation indicators, and desktop integration (icon and `.desktop` entry for packaged builds).

## Install

- **Arch / AUR:** Use the `PKGBUILD-bin` template against this tag’s GitHub release assets when publishing to the AUR (`dev/scripts/aur-push.sh` after updating the AUR `PKGBUILD`).
- **From source:** See the repository README for `cargo build` / `cargo run` on Arch with GTK 4 and libadwaita 1.6+.

## Thanks

Early testers and contributors who reported rough edges in the first public iteration—your feedback shapes the next releases.
