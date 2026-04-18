# Release v0.1.0

First tagged release of **aur-pkgbuilder**: a GTK4 / libadwaita desktop app for AUR maintainers to sync PKGBUILDs, bump versions, validate, build, and publish from one place.

## Highlights

- **End-to-end workflow:** Home, connection checks, package sync, version step, validation, `makepkg` build, and AUR Git publish—with streaming logs in the app.
- **PKGBUILD editing:** In-app editor with quick fields, diff-friendly updates, and safer handling around `updpkgsums` when checksums already match.
- **SSH & AUR:** Guided AUR SSH key setup, `known_hosts` fingerprint surfacing, preflight probes, and publish gating when SSH is not verified. Saving your AUR username runs an RPC check; Home highlights registry packages that are not under that account so you can clean them up in bulk.
- **Bootstrap checks:** When you add a new package, the app validates **pkgbase** naming, explains pkgbase vs split `pkgname`, and probes the AUR plus official repos so you do not pick a colliding name by mistake.
- **Connection & environment:** Required-tool detection, richer **Recommended environment** rows (including `base-devel` and devtools hints), and shortcuts to open common packaging config paths with your default app.
- **Packages & paths:** JSONC config and registry, optional per-package destinations, validated paths, and folder pickers rooted on the window.
- **Shell & UX:** Tabbed main shell with workflow navigation, connection/validation indicators, and desktop integration (icon and `.desktop` entry for packaged builds).

## Install

- **Arch / AUR:** Use the `PKGBUILD-bin` template against this tag’s GitHub release assets when publishing to the AUR (`dev/scripts/aur-push.sh` after updating the AUR `PKGBUILD`).
- **From source:** See the repository README for `cargo build` / `cargo run` on Arch with GTK 4 and libadwaita 1.6+.

