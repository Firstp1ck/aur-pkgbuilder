# aur-pkgbuilder

[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)
[![Made with Rust](https://img.shields.io/badge/Made%20with-Rust-orange.svg)](https://www.rust-lang.org/)
[![Target: Arch Linux](https://img.shields.io/badge/Target-Arch%20Linux-1793D1?logo=arch-linux&logoColor=white)](https://archlinux.org/)
[![Toolkit: GTK4](https://img.shields.io/badge/Toolkit-GTK4-4A86CF?logo=gnome&logoColor=white)](https://gtk.org/)
[![UI: libadwaita](https://img.shields.io/badge/UI-libadwaita-3584E4)](https://gitlab.gnome.org/GNOME/libadwaita)

[![x86_64](https://img.shields.io/badge/CPU-x86__64-blue.svg)](.)
[![aarch64](https://img.shields.io/badge/CPU-aarch64-blue.svg)](.)

aur-pkgbuilder is a GTK4/libadwaita desktop application that walks a maintainer through building and publishing an Arch User Repository package end-to-end: sign in with your AUR username and pull the packages you maintain, set up SSH for git, sync a PKGBUILD from its upstream source, run the standard AUR checks, build with `makepkg`, then commit and push to the AUR git remote. **Register new AUR package** on Home covers the first-time `git push` that creates a new pkgbase on the AUR when you do not already have a repo there; **Publish** stays the path for existing packages. Each package can keep its tree under the working directory (default folder matches the package id, or a custom relative path), or you can point it at any absolute folder on disk when you already maintain the PKGBUILD outside the work directory. Packages are data-driven through an editable registry, so new AUR packages can be added and administered entirely from the GUI.

## Community

Idea or bug? Open an issue on the project tracker. Contributions are welcome — see [Contributing](#contributing) below.

## Supported Platforms

| Supported Distributions | Supported Desktops |
|:---|:---|
| [![Arch Linux](https://img.shields.io/badge/Arch%20Linux-1793D1?logo=arch-linux&logoColor=white)](https://archlinux.org/) | [![GNOME](https://img.shields.io/badge/GNOME-4A86CF?logo=gnome&logoColor=white)](https://www.gnome.org/) |
| [![EndeavourOS](https://img.shields.io/badge/EndeavourOS-1793D1?logo=endeavouros&logoColor=white)](https://endeavouros.com/) | [![KDE Plasma](https://img.shields.io/badge/KDE%20Plasma-1D99F3?logo=kde&logoColor=white)](https://kde.org/) |
| [![CachyOS](https://img.shields.io/badge/CachyOS-1793D1?logo=arch-linux&logoColor=white)](https://cachyos.org/) | [![Wayland](https://img.shields.io/badge/Wayland-FFB300)](https://wayland.freedesktop.org/) |
| [![Manjaro](https://img.shields.io/badge/Manjaro-35BF5C?logo=manjaro&logoColor=white)](https://manjaro.org/) | [![X11](https://img.shields.io/badge/X11-F28834)](https://www.x.org/) |
| [![Artix](https://img.shields.io/badge/Artix-1793D1?logo=arch-linux&logoColor=white)](https://artixlinux.org/) | |

## Table of Contents
- [Quick start](#quick-start)
- [Features](#features)
- [Screenshots](#screenshots)
- [Usage](#usage)
- [Configuration](#configuration)
- [Troubleshooting](#troubleshooting)
- [Roadmap](#roadmap)
- [Credits](#credits)
- [License](#license)

## Quick start

Pick the install method that fits your use case — all three produce the same `aur-pkgbuilder` binary:

| Method | Command | Build step? | Best for |
|--------|---------|-------------|----------|
| [AUR helper](#install-via-an-aur-helper) | `paru -S aur-pkgbuilder-bin` | none — prebuilt | end users |
| [Cargo](#install-via-cargo) | `cargo install --git https://github.com/Firstp1ck/aur-pkgbuilder --locked` | compiles locally | quickly trying main |
| [From source](#build-from-source) | `git clone … && cargo run --release` | compiles locally | contributors |

### Runtime dependencies

Every install method needs these Arch packages on the system so the app can actually drive a release:

```bash
sudo pacman -S --needed \
    gtk4 libadwaita \
    base-devel git openssh pacman-contrib xdg-utils
```

Optional — used by the Validate step; missing tools turn into *skipped* rows with an install hint:

```bash
sudo pacman -S --needed namcap shellcheck
```

The AUR helper path pulls the required ones automatically through the PKGBUILD's `depends=()`. For Cargo and from-source you need to install them yourself.

### Build dependencies (Cargo and from-source only)

```bash
sudo pacman -S --needed rustup pkgconf
rustup default stable
```

`gtk4` and `libadwaita` from the runtime list double as build-time headers via `pkg-config`, so no separate `-devel` package is needed.

### Install via an AUR helper

```bash
paru -S aur-pkgbuilder-bin       # or: yay -S aur-pkgbuilder-bin
```

The `aur-pkgbuilder-bin` package ships prebuilt x86_64 and aarch64 binaries from the upstream GitHub release (see [`PKGBUILD-bin`](PKGBUILD-bin)). No Rust toolchain is needed on your machine.

### Install via Cargo

```bash
cargo install --git https://github.com/Firstp1ck/aur-pkgbuilder --locked
```

The binary lands in `~/.cargo/bin/aur-pkgbuilder`. Make sure `~/.cargo/bin` is on your `PATH` (most distro-provided `rustup` setups already do this).

### Build from source

```bash
git clone https://github.com/Firstp1ck/aur-pkgbuilder
cd aur-pkgbuilder
cargo run --release
```

For a debug run during development, drop `--release`.

### First launch

There are two distinct AUR identities this app cares about, and they show up in two separate screens:

| Identity | What it is | Where it's set |
|---|---|---|
| **Login (username)** | Lightweight identifier for the AUR RPC. Tells the app which packages to list and which maintainer role you have. No password. | Onboarding screen (first launch). |
| **Verification (SSH key)** | Cryptographic proof that you are that username when you push a release. This is what the AUR actually checks. | Connection / SSH setup screens. |

1. Enter your aur.archlinux.org username on the onboarding screen — this is your *login*. The app queries the public AUR RPC for every package where you're the maintainer or a co-maintainer and shows a checklist with role badges and out-of-date flags.
2. Tick the packages you want to administer and press **Import & continue to SSH**. The app imports the picks into the registry and pushes you straight into the SSH setup step.
3. On the SSH setup screen, press **Run setup** for the one-click flow (creates `~/.ssh/aur`, writes the `Host aur.archlinux.org` block, pins the server host keys), then **Finish onboarding** to return home.
4. Run **Test SSH connection** on the AUR connection screen — this is the *verification* step that proves the username belongs to you. Until this probe passes in the current session, the Publish step blocks commit/push behind a banner.
5. Walk through Sync → Version → Validate → Build → Publish (or use **Register new AUR package** on Home when the AUR Git repo does not exist yet — that wizard validates, prepares the tree, and runs the initial push once SSH is verified).

**Skip setup** on the onboarding screen leaves the registry empty and SSH unconfigured — the app still opens cleanly, you can edit PKGBUILDs and run local builds, but the Publish step will keep its "SSH is not verified" banner until you set SSH up and run the probe.

The onboarding is always reachable again from **Import from AUR account…** on the home page.

An SSH key registered on [aur.archlinux.org](https://aur.archlinux.org/) is required for **Publish** and for the Register wizard’s remote steps. For packages that already have an AUR repo, use Publish; for a brand-new pkgbase, use **Register new AUR package** on Home.

## Features

| Feature | Description |
|---------|-------------|
| **Guided wizard** | libadwaita tabs plus `NavigationView` overlays take you through onboarding (username → SSH setup) → home → AUR connection → sync → version → validate → build → publish (and **Manage packages**). **Register new AUR package** opens as an overlay from Home. Each step has clear prerequisites and surfaces errors as toasts. |
| **AUR login (username)** | Enter your AUR username once; the app queries the public AUR RPC for every package you maintain or co-maintain and imports the ones you pick. No passwords — the RPC is read-only, and the username is just an identifier. |
| **AUR verification (SSH)** | One-click SSH setup: creates (or reuses) `~/.ssh/aur`, writes the `Host aur.archlinux.org` block into `~/.ssh/config`, and pins the server's host key into `~/.ssh/known_hosts`. Each step is also available on its own. Existing files are never overwritten. |
| **Editable package registry** | Packages live in `~/.config/aur-pkgbuilder/packages.jsonc` (JSONC — comments allowed). Add, edit, and remove entries from the GUI; nothing about a specific package is hardcoded in source. Optional `destination_dir` is an absolute path to that package’s PKGBUILD/build tree; otherwise optional `sync_subdir` picks a folder under the working directory (default: package id). |
| **Preflight checks** | Detects `makepkg`, `git`, `ssh`, and `updpkgsums` on `PATH` and shows install hints for missing ones. A non-interactive `ssh -T aur@aur.archlinux.org` probe confirms your key is accepted by the AUR. Only the Publish step is gated on the probe — sync / version / validate / build run fine without SSH. When the probe result changes, the app refreshes publish-related UI so you are not stuck behind a stale banner. |
| **PKGBUILD sync** | Downloads the upstream `PKGBUILD` from each package’s URL into that package’s folder — either an absolute `destination_dir` or `<work_dir>/` + relative path (default `<pkg_id>`; optional `sync_subdir` e.g. `group/pkg` for a shared tree). |
| **Checksum refresh** | One-click `updpkgsums` with streamed output. Useful for binary/source packages after a version bump; a no-op for git packages with empty `source=`. |
| **Standard PKGBUILD validation** | Runs the checks an AUR maintainer runs by hand — `bash -n PKGBUILD`, `makepkg --printsrcinfo`, `makepkg --verifysource`, plus optional `shellcheck` and `namcap`. Each check reports pass / warn / fail / skipped with a streaming log. Missing optional tools surface an install hint instead of failing. |
| **Extended fakeroot validation** | A separate button runs `makepkg -f --noconfirm` (which exercises the full build including the fakeroot-backed `package()` step) and then `namcap -i` on the resulting `.pkg.tar.*`. Catches issues that only show up during real packaging — missing file permissions, wrong deps, empty `package()`, etc. |
| **Live build log** | `makepkg -f` runs asynchronously with stdout/stderr streamed line-by-line into a monospace log view. Optional `--nobuild` and `--clean` toggles. |
| **Root safety** | Refuses to build as root to match `makepkg`'s own policy. |
| **AUR git publish** | Clones `ssh://aur@aur.archlinux.org/<pkg_id>.git` on demand, regenerates `.SRCINFO`, shows a `git diff` preview, then commits and pushes with an editable commit message. |
| **Default commit message** | Set a reusable template (supports `{pkg}` as the package name) via **Save as default** on the publish screen. Every subsequent commit opens with that template pre-filled and rendered, so you see the default and can edit before pushing. **Reset to default** re-renders the saved template if you change your mind mid-edit. |
| **Home package list** | Search, sort, and filter the registry list; favorites stay grouped at the top. After you save your AUR username on the Connection tab, **Remove mismatched…** drops packages that failed the last maintainer/co-maintainer RPC check (shown in red in the list). |
| **Register on AUR** | From Home, **Register new AUR package** runs a wizard: define the pkgbase, create a starter `PKGBUILD` or edit an existing tree, validate, **Prepare**, then the initial **push** to create the AUR Git repository (with optional handling when the remote already has history). A remote `PKGBUILD` probe avoids overwriting an existing AUR tree by mistake. |
| **Administration screen** | **Manage packages** lists every entry with per-row actions (including **Check upstream** — compares the on-disk `PKGBUILD` to the configured upstream URL with a diff when they differ). **Import from existing AUR repo** and bulk **check all** remain `preview` stubs; **AUR SSH commands** opens from here. |
| **AUR SSH commands** | Dedicated page that exposes the commands `aur@aur.archlinux.org` accepts — `help`, `list-repos`, `vote`, `unvote`, `flag`, `unflag`, `notify`, `unnotify`, `adopt`, `disown`, `setup-repo`, `set-comaintainers`, `set-keywords`. Destructive commands are clearly tagged; output streams into a shared log. |
| **Persistent settings** | Working directory and SSH key path are stored at `~/.config/aur-pkgbuilder/config.jsonc` (JSONC — comments allowed). |

## Screenshots

### Register new AUR package

Wizard to register a new AUR package: pkgbase and folder, remote `PKGBUILD` probe, starter or edited `PKGBUILD`, then validate / prepare / initial push.

![Register wizard — define package and main actions](images/Register_AUR_part1_v0.1.0.png)

![Register wizard — PKGBUILD editing](images/Register_AUR_part2_v0.1.0.png)

![Register wizard — prepare clone and push to AUR](images/Register_AUR_part3_v0.1.0.png)

### Home

Registered packages, **Add package…**, **Manage packages…**, and **Import from AUR account…**.

![Home — package registry and primary actions](images/Home_v0.1.0.png)

### AUR connection

Preflight tools, working directory, SSH key, and **Test SSH connection**.

![AUR connection — verify environment and SSH](images/Connection_v0.1.0.png)

### Sync PKGBUILD

Upstream URL, destination folder, and download into the resolved tree.

![Sync PKGBUILD from upstream](images/Sync_v0.1.0.png)

### Version and checksums

PKGBUILD editor, quick fields, diff, and **Run updpkgsums**.

![Version — editor and checksums (1)](images/Version_part1_v0.1.0.png)

![Version — editor and checksums (2)](images/Version_part2_v0.1.0.png)

### Validate

Required checks, optional lints, extended fakeroot build, and shared log.

![Validate — PKGBUILD checks and log](images/Validate_v0.1.0.png)

### Build

`makepkg` with toggles and streaming output.

![Build — makepkg and build log](images/Build_v0.1.0.png)

### Publish

`.SRCINFO`, diff preview, commit message, and **Commit and push**.

![Publish — git diff and push to AUR](images/Publish_v0.1.0.png)

### Manage packages

Administration surface: lifecycle actions, per-package menu, and entry to **AUR SSH commands**.

![Manage packages — administration view](images/Manage_v0.1.0.png)

### AUR SSH commands

Curated `aur@aur.archlinux.org` command groups with per-row **Run** and shared log.

![AUR SSH commands — account and voting groups](images/AUR_SSH_Commands_part1_v0.1.0.png)

![AUR SSH commands — maintenance and metadata](images/AUR_SSH_Commands_part2_v0.1.0.png)

## Usage

Each screen of the wizard is self-contained and documents what it will run.

**0. Onboarding — sign in + set up SSH** (first launch / **Import from AUR account…**) — Your username is the *login*; the AUR RPC uses it to list packages where you're maintainer or co-maintainer. Tick the ones you want to administer and press **Import & continue to SSH**. Imported packages land in the registry with a PKGBUILD URL pointing at the AUR's cgit plain view, and the app immediately pushes you to the SSH setup step — **Run setup** there is a single button that creates `~/.ssh/aur`, writes the SSH config entry, and populates `known_hosts`. **Finish onboarding** returns to the home screen. **Skip setup** at any point is allowed; Publish will stay gated until you come back and finish SSH.

**1. Home** — Above the list: **Search**, **Sort**, and **Filter** (including whether to hide rows that failed the last AUR-account RPC check). Registered packages appear as rows with edit (pencil) and remove (trash); favorites are grouped first. Action row:

- **Add package…** — register a package by hand (AUR pkgname, raw PKGBUILD URL, kind).
- **Register new AUR package** — wizard for a new pkgbase: starter or custom `PKGBUILD`, validate, prepare clone, initial push (needs verified SSH).
- **Manage packages…** — administration view (upstream checks, SSH commands; some lifecycle buttons still `preview`).
- **Import from AUR account…** — re-enter the onboarding to add more packages from your AUR profile.
- **Remove mismatched…** — bulk-remove packages not listed for your saved AUR username as maintainer or co-maintainer after the last Connection-tab check (destructive; confirms first).

**2. AUR connection — verify with SSH** — Lists required tools with install hints, lets you set the working directory (type a path or use **Browse…**), pick an SSH key, and runs the SSH probe. Toasts confirm saves and other async results on this screen. This is the step that *verifies* the username you entered on onboarding actually belongs to you. **Continue is always available** — sync / build / validate don't need SSH, only Publish does; a failed probe doesn't block the rest of the wizard. The **Set up SSH…** sub-page runs the concrete setup:

- **One-click setup** — creates or reuses `~/.ssh/aur` (ed25519), adds a `Host aur.archlinux.org` block to `~/.ssh/config`, and pins the server's host keys into `~/.ssh/known_hosts`. Safe to click repeatedly; nothing is overwritten.
- Individual buttons let you run each step in isolation, copy the public key to the clipboard, or open the AUR account page.

**3. Sync PKGBUILD** — Shows the upstream URL and where files land. You can keep the default under the working directory, set a **relative** folder (under the work dir; blank uses the package id), or set an **absolute** destination and browse to it — the editor and all later steps use that resolved folder. Save to the registry, then download the PKGBUILD on click.

**4. Version and checksums** — Kind-specific guidance (binary vs git vs source), an in-app PKGBUILD editor (quick fields plus the full file), optional **diff** against the last saved copy, and a gentle **stale** hint when the tree has not been synced or reloaded from disk in a while. There is also a generic **Run updpkgsums** button with its own streaming log.

**5. Validate** — Runs the standard PKGBUILD checks with per-check status icons and a shared log pane, split into three tiers:

- *Required*: `bash -n PKGBUILD` (syntax), `makepkg --printsrcinfo` (metadata parses), `makepkg --verifysource` (sources fetch and checksum).
- *Optional lints*: `shellcheck -s bash -S warning PKGBUILD` and `namcap PKGBUILD`. Missing tools are reported as *skipped* with an install hint (`pacman -S --needed shellcheck` / `namcap`), not as a failure.
- *Extended (fakeroot build)*: `makepkg -f --noconfirm` — a full build that exercises the fakeroot-backed `package()` step and produces a real `.pkg.tar.*` — followed by `namcap -i <pkg>` on the resulting artefact. Slow (minutes for complex packages), so it has its own **Run extended checks** button.

Use **Run all checks** for the fast tiers, **Run extended checks** for the fakeroot build, or each row's **Run** button for targeted re-runs. A failing required check does not lock navigation — you can still proceed, but the toast warns you.

**6. Build** — Runs `makepkg -f` in the package directory. Toggle `--nobuild` or `--clean` as needed. Output streams to the log view; a toast announces success or failure.

**7. Publish** — Clones (or reuses) the AUR git repo under `<work_dir>/aur/<pkg_id>`, regenerates `.SRCINFO`, copies the new PKGBUILD into place, and shows `git diff`. Review, adjust the commit message, press **Commit and push**.

This step needs a verified SSH connection. If you haven't run **Test SSH connection** on the connection screen in the current session, the Publish page shows an **SSH is not verified** banner with a direct link to the SSH setup sub-page, and both **Prepare** and **Commit and push** stay disabled until the probe succeeds. You can still edit the PKGBUILD, build locally, and regenerate `.SRCINFO` while SSH is unverified — only the remote git operations are gated.

The commit-message field is pre-filled from your saved default template (fallback: `{pkg}: update`). The "Default template" row below the field shows the current default. **Save as default** stores whatever's in the field as the new template — if you typed the current package name literally, it's de-substituted back to `{pkg}` so the template keeps working across packages. **Reset to default** reloads the template and re-renders it for the current package.

**Manage packages** (from the home page) — **Register new AUR package** lives on **Home**, not here. This screen groups lifecycle and SSH shortcuts in collapsible sections, then the full package list:
- `Import from existing AUR repo` *(preview)* — clone by AUR pkgname and pre-fill a registry entry.
- `Check all packages for upstream updates` *(preview)* — run upstream comparison for every package in one go.
- **AUR SSH commands** — opens the curated command picker (see below).
- Per-row menu: open build wizard, open working directory (functional, via `xdg-open`), **Check upstream** (live — fetches the configured PKGBUILD URL and compares to disk), archive / disown *(preview)*.

**AUR SSH commands** (from **Manage packages → Open**) — Curated picker for the subset of commands `aur@aur.archlinux.org` accepts. The page shares one package-name and one extra-args field across four groups:

- *Account*: `help`, `list-repos` (read-only; ignore the package field).
- *Voting & notifications*: `vote`, `unvote`, `flag [reason]`, `unflag`, `notify`, `unnotify`.
- *Maintenance* (tagged **destructive**): `adopt`, `disown`, `setup-repo`.
- *Package metadata*: `set-comaintainers <users…>`, `set-keywords <keywords…>`.

Each row has its own **Run** button. Output is streamed into a shared log pane. The SSH key configured on the connection screen is used automatically.

## Configuration

All state lives under `~/.config/aur-pkgbuilder/` as **JSONC** (JSON with Comments) — both `//` line comments and `/* */` block comments are accepted on read, and each saved file is prefixed with a fixed header explaining the schema. The legacy `.json` files are still read on first load and replaced with their `.jsonc` equivalents on the next save:

- `config.jsonc` — selected working directory, SSH key path, last-opened package, cached AUR username, default commit-message template.
- `packages.jsonc` — the package registry. Each entry is an object with:

```jsonc
// aur-pkgbuilder package registry (JSONC — // and /* */ comments are allowed)
{
  "version": 1,
  "packages": [
    {
      "id": "my-pkg-bin",
      "title": "My Package (binary)",
      "subtitle": "Short description shown on the home card.",
      "kind": "bin", // "bin" | "git" | "other"
      "pkgbuild_url": "https://example.com/raw/PKGBUILD-bin",
      "icon_name": null,
      "destination_dir": null, // optional absolute folder for this package's tree; null = under work_dir
      "sync_subdir": null // optional relative folder under work_dir when destination_dir is null; null = use id
    }
  ]
}
```

Build and sync artefacts live either in the package’s `destination_dir` (absolute path) or under `<work_dir>/<folder>/`, where `<folder>` is the package id unless `sync_subdir` sets a safe relative path. AUR git clones stay under `<work_dir>/aur/<pkg_id>/`. The default `<work_dir>` is `$XDG_CACHE_HOME/aur-pkgbuilder/builds`.

Both files are safe to hand-edit — comments outside the JSON object block persist across saves, but comments placed inside the JSON body are overwritten the next time the GUI saves.

## Troubleshooting

- **SSH probe reports "key rejected"** — the tested key is not registered on aur.archlinux.org. Use the SSH key override field to point at the correct key, then re-probe.
- **SSH probe reports "failed" with a host-key error** — the first connection needs to accept the `aur.archlinux.org` host key. The app passes `StrictHostKeyChecking=accept-new`, but a stale entry in `~/.ssh/known_hosts` will still block it. Remove the old entry with `ssh-keygen -R aur.archlinux.org` and re-probe.
- **"Refusing to build as root"** — `makepkg` cannot run as root. Re-launch the GUI as your normal user.
- **`updpkgsums: command not found`** — install `pacman-contrib`.
- **Nothing happens after "Commit and push"** — inspect the publish log pane; an unhelpful exit code usually means the remote rejected the push (fast-forward required, wrong key, or unregistered package).
- **Package registry looks corrupt** — delete `~/.config/aur-pkgbuilder/packages.jsonc` (and the legacy `packages.json` if present); the next launch starts with an empty registry and you can re-add entries from the UI.

## Roadmap

The core wizard is feature-complete for day-to-day releases, including **Register new AUR package** from Home. Some administration actions are still `preview` stubs; they surface "coming soon" toasts until implemented.

### Tracked (preview)

- **Import from an existing AUR repo** — clone by pkgname and parse the PKGBUILD to pre-fill a registry entry.
- **Check upstream for updates** — compare local vs upstream `pkgver` per package, with a bulk "check all" action.
- **Archive / disown** — automate the AUR web RPC for `/packages/<id>/disown/`.

### Other potential features

- Clean-chroot builds via `devtools` (`extra-x86_64-build`).
- Embedded VTE terminal for true `makepkg` interactivity.
- Automatic GitHub Release drafting for binary packages after a push.
- Per-package `install` file support (`.install` hook) and extra sources.
- Pacman-style vercmp for the update check (replacing the MVP lexical compare).
- Dark/light theme override independent of the system style.

## Credits

- Built with [gtk4-rs](https://gtk-rs.org/gtk4-rs/) and [libadwaita-rs](https://gtk-rs.org/gtk4-rs/stable/latest/docs/libadwaita/)
- Async subprocesses via [Tokio](https://tokio.rs/)
- PKGBUILD fetch via [reqwest](https://docs.rs/reqwest) over rustls
- Powered by Arch + AUR

## License

MIT — see [LICENSE](LICENSE).

## Contributing

Contributions are welcome. Fork the repo, open a pull request, and keep the MVP scope in mind — remaining `preview` administration actions should land one at a time with matching UX updates. From the repository root, `make help` lists `fmt`, `clippy`, `test`, and other shortcuts that mirror local checks.
