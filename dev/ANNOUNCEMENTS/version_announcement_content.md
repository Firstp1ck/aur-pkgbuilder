## What's New

- **End-to-end workflow** — Home, connection checks, package sync, version step, validation, `makepkg` build, and AUR Git publish, with streaming logs in the app.
- **PKGBUILD editing** — In-app editor with quick fields, diff-friendly updates, and safer `updpkgsums` handling when checksums already match sources.
- **SSH and AUR** — Guided AUR SSH key setup, `known_hosts` fingerprint surfacing, preflight probes, and publish gating when SSH is not verified. Saving your AUR username runs an RPC check; Home flags registry packages not under that account for bulk cleanup.
- **Bootstrap checks** — New packages validate **pkgbase** naming, explain pkgbase vs split `pkgname`, and probe the AUR plus official repos to avoid colliding names.
- **Connection and environment** — Required-tool detection, richer **Recommended environment** hints (`base-devel`, devtools), and shortcuts to open common packaging config paths.
- **Packages and paths** — JSONC config and registry, optional per-package destinations, validated paths, and folder pickers rooted on the window.
- **Shell and UX** — Tabbed main shell with workflow navigation, connection/validation indicators, and packaged integration (icon and `.desktop` entry).

