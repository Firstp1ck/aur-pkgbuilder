# Designing a Standalone GUI to Guide Maintainers Through the Full AUR Package Lifecycle

## Overview

This document specifies the functionality, flows, and architecture for a standalone desktop GUI that guides an Arch Linux user through the entire AUR maintenance lifecycle, from initial SSH/key setup and repository bootstrap to local build/test (including clean chroot) and finally publishing updates to the AUR via git.[^1][^2][^3]
It assumes the user is comfortable with Arch packaging concepts but may not remember exact commands, flags, or best practices for every step.[^4][^5]
The goal is to identify all necessary capabilities and integration points so an existing in-development GUI can be audited for gaps and extended.

## High‑Level Workflow

At a high level, the GUI should model the maintainer workflow as a series of stages:

1. Environment and identity setup (AUR account, SSH keys, git identity)
2. Local packaging environment (base-devel, makepkg configuration, devtools/clean chroot tooling)
3. Package project bootstrap (new or existing AUR package)
4. PKGBUILD authoring and validation
5. Local build and install test (makepkg, pacman -U)
6. Optional clean chroot build (devtools or wrapper tools)
7. .SRCINFO generation and consistency checks
8. Git commit and push to AUR (initial upload and subsequent updates)
9. Maintenance tasks (adoption, orphaning, version bumps, pkgrel bumps, review requests)

Each stage should be represented in the UI as a clearly visible step or wizard section, with contextual guidance, status indicators, and actionable buttons for the relevant commands.[^3][^1]

## Stage 1: AUR Identity and SSH Setup

### Requirements

The GUI needs to verify and, where possible, assist with the following:

- AUR account existence: guide the user to register/log in on the AUR web interface; this is not automatable but should be linked from the GUI.[^4]
- SSH key pair for AUR pushing:
  - Generate a dedicated keypair (e.g., `~/.ssh/aur`), using `ssh-keygen -t rsa -b 4096 -C "email"` or ed25519, depending on user choice.[^2]
  - Display the public key content and provide a button to copy it for pasting into the AUR web account profile.[^2]
- AUR SSH host configuration:
  - Optionally create or update `~/.ssh/config` with a `Host aur` block pointing to `aur.archlinux.org`, `User aur`, and `IdentityFile ~/.ssh/aur` or user‑chosen key path.[^2]
- SSH connectivity test:
  - Provide a "Test AUR SSH" button that runs `ssh aur` or `ssh aur@aur.archlinux.org` in a controlled way and shows the result.

### GUI Elements

- Setup wizard page with status checks:
  - "AUR account configured" (manual confirmation after opening a browser)
  - "SSH key pair present" (detect via filesystem)
  - "Public key registered on AUR" (cannot be fully verified, but ssh test can give confidence)
  - "SSH host config present" (optional, with auto‑config button).
- Log/console pane for showing SSH test output.

## Stage 2: System and Packaging Tooling Setup

### Requirements

The GUI should verify that the system has the required packaging and build tooling:

- `base-devel` group installed, as recommended by the Arch Wiki for building from the AUR.[^4]
- `git` installed (needed for AUR repositories and VCS packages).[^4]
- `pacman` and `makepkg` available (core system components).[^6]
- `devtools` installed for clean chroot building, including scripts like `extra-x86_64-build` and `multilib-build`.[^3]
- Optionally, tools like `pkgbuild-introspection` for `.SRCINFO` generation (`mksrcinfo`), though `makepkg --printsrcinfo` is typically preferred.[^7][^2]
- Optional wrappers such as `clean-chroot-manager` if the GUI wants to integrate with them.[^8]

The GUI should also surface relevant configuration files:

- `/etc/makepkg.conf` and any architecture‑specific configs from devtools such as `/usr/share/devtools/makepkg-x86_64.conf`.[^9][^6]
- Pacman configuration files used by devtools for chroots, e.g., `/usr/share/devtools/pacman.conf.d/*.conf`.[^3]

### GUI Elements

- "Environment check" page that runs detection commands and presents a checklist with remediation hints.
- Buttons to open configuration files in the user’s preferred editor (configurable path) for `makepkg.conf` and related.
- Suggestions/tips panel explaining why clean chroot builds and `base-devel` are recommended.[^3][^4]

## Stage 3: Package Project Bootstrap

### Requirements

The GUI must support two main bootstrap flows:

1. **Create a new AUR package** (name not yet in AUR):
  - Check that the proposed package name is not already present in AUR (via HTTP query to the AUR web API or by opening the package page in a browser).[^1][^4]
  - Enforce AUR rules: no duplication of packages already in official repositories, unless clearly differentiated (e.g., `-git`, extra features, patches, conflicts array).[^1]
  - Initialize a new local git repository with the proper remote pointing to `ssh://aur@aur.archlinux.org/pkgname.git` (or using the `Host aur` alias).[^7][^2]
  - Create a minimal `PKGBUILD` template based on package type (simple source, VCS with `-git`, language‑specific templates like Rust, Python) while staying within Arch packaging and submission guidelines.[^5][^1]
2. **Work on an existing AUR package**:
  - Clone via `git clone ssh://aur@aur.archlinux.org/pkgname.git` or using the `aur` host alias.[^7][^2]
  - Detect and import existing `PKGBUILD`, `.SRCINFO`, and ancillary files.
  - Detect whether the user is maintainer, co‑maintainer, or just someone working locally (the GUI may need to rely on git push errors or web page info for this).

### GUI Elements

- "New or existing" selection with:
  - New package wizard (name, description, URL, license, etc.)
  - Existing package search and clone helper.
- Package list view of locally tracked AUR packages with path, current version, and last sync status.

## Stage 4: PKGBUILD Authoring and Validation

### Requirements

PKGBUILD authoring is central; the GUI must respect that a PKGBUILD is a Bash script that follows Arch’s packaging guidelines.[^5]
The GUI should not hide this fact but should provide guardrails and helpers.

Necessary capabilities:

- Syntax‑aware editor for `PKGBUILD` with:
  - Bash syntax highlighting.
  - Snippet/Template insertion for common fields (`pkgname`, `pkgver`, `pkgrel`, `arch`, `url`, `license`, `source`, `sha256sums`, `depends`, `makedepends`, `pkgver()` for VCS packages, etc.).[^5]
  - Language‑specific helpers (e.g., Rust, Python, Go) that add conventional `makedepends` and build steps, derived from Arch packaging wiki pages.
- Basic static checks:
  - Required fields present.
  - `pkgname` matches AUR repo name.
  - `pkgrel` increment logic (e.g., warn if `pkgrel` was not bumped when making non‑`pkgver` changes).[^7]
  - Use of arrays like `depends`, `makedepends`, `conflicts`, `provides`, `replaces` consistent with guidelines.[^1][^5]
- Integration with lint tools (if available) or embedding custom heuristics based on AUR submission guidelines: duplicate of official packages, mis‑named VCS packages, etc.[^1]
- Provide a button to open relevant ArchWiki pages (`PKGBUILD`, `makepkg`, `AUR submission guidelines`) for quick reference.[^6][^5][^1]

### GUI Elements

- Central editor panel with sidebars:
  - Metadata sidebar summarizing key fields and validation status.
  - Inline diagnostics pane listing warnings and errors.
- Quick‑fix suggestions where safe, e.g., "Add `conflicts=('screen')` for patched screen variant" as an example pattern from guidelines.[^1]

## Stage 5: Local Build and Install Test

### Requirements

Once a PKGBUILD exists, the GUI should manage the standard local build workflow using `makepkg`.[^6]

Core capabilities:

- Configure build options per project or globally (e.g., `-s` to sync deps, `-c` to clean up, `-f` to force rebuild, `--noconfirm`, etc.).[^6]
- Run `makepkg` in the package directory and stream logs into a console panel.
- Detect success/failure and show the resulting `.pkg.tar.`* files.
- Offer to install the resulting package via `pacman -U` (with safe prompts for root elevation and confirming operations).
- Show information about installed version vs PKGBUILD `pkgver`/`pkgrel` to confirm tests matched what is being published.

### GUI Elements

- "Build" tab per package with:
  - Build options form (checkboxes for common flags, and an advanced text field for custom flags).
  - Log/console window capturing `makepkg` output.
  - Status bar showing last build result and timestamp.
- "Install for testing" button that uses `pacman -U` on a selected artifact.

## Stage 6: Clean Chroot Build Integration

### Requirements

Building in a clean chroot is recommended to detect missing dependencies and ensure clean linkage, and is standard practice using devtools.[^9][^3]

The GUI should support:

- Detection of devtools and related scripts like `extra-x86_64-build`, `multilib-build`, etc., and the underlying makechrootpkg mechanism.[^10][^3]
- Configuration of a working directory for chroot matrices (often `/var/lib/archbuild`) and ensuring necessary permissions.[^3]
- Single‑click "Build in clean chroot" action that:
  - Picks the correct devtools build script based on architecture/repo context (most AUR packages will target extra/multilib style environment).[^3]
  - Executes the script in the package directory.
  - Shows logs and result artifacts.
- Optional integration with a wrapper such as `clean-chroot-manager` for users who already rely on it: the GUI could offer alternative backend selection (devtools direct vs wrapper).[^8][^9]

### GUI Elements

- Chroot configuration dialog:
  - Path to chroot matrix.
  - Toggle for resetting chroot (`-c` flag) as needed.[^3]
- Chroot build tab very similar to local build, but clearly labeled, with environment and config summary.

## Stage 7: .SRCINFO Management

### Requirements

`.SRCINFO` describes package metadata and must be present and up‑to‑date in every commit pushed to AUR.[^11][^2][^7]

The GUI needs to ensure:

- `.SRCINFO` exists in the repository root before any git commit destined for AUR.
- `.SRCINFO` matches the current PKGBUILD.

Supported generation methods:

- Run `makepkg --printsrcinfo > .SRCINFO` in the package directory (preferred, modern approach).[^11][^7]
- Optionally support `mksrcinfo` from `pkgbuild-introspection` (for historical compatibility), but makepkg is usually sufficient.[^2]

The GUI must provide:

- A "Regenerate .SRCINFO" button.
- Automatic regeneration hook on significant PKGBUILD changes (for example, before a commit or on explicit build/publish flows).
- A diff view showing `.SRCINFO` changes before commit.

### GUI Elements

- Status indicator in the package overview: "SRCINFO in sync" or "SRCINFO out of date".
- Button in a toolbar or dedicated metadata panel to regenerate and view `.SRCINFO`.

## Stage 8: Git Commit and Push to AUR

### Requirements

The GUI needs reliable git integration tailored to AUR flows:[^12][^2][^7]

- Repository initialization for new packages (already covered in Stage 3) and remote configuration pointing to AUR.
- Ensure user.name and user.email are set for git; provide settings or detect global config and warn if missing.[^12]
- Stage relevant files only, typically:
  - `PKGBUILD`
  - `.SRCINFO`
  - Supporting files (`.install`, `.service`, patches, etc.)
  - `.gitignore` with standard contents (e.g., ignoring built artifacts, but whitelisting PKGBUILD, .SRCINFO, patches, etc.).[^7]
- Provide commit message helper (templates like "Update to pkgver" or "Bump pkgrel").
- Perform git commit and show result; handle cases where commit fails due to missing identity.[^12]
- Push to AUR remote and show remote output; parse common errors (e.g., non‑fast‑forward, permission denied) and offer hints.

For subsequent uploads:

- Enforce or at least encourage incrementing `pkgrel` when making changes that do not change `pkgver`, as per standard maintenance practice.[^7]
- Regenerate `.SRCINFO` before commit.

### GUI Elements

- Git panel and history viewer showing recent commits.
- Simple staging UI with file list and checkboxes.
- Push button with clear indication of target remote (AUR) and branch (usually master/main).

## Stage 9: Maintenance and Review Workflows

### Requirements

Beyond initial publishing, a maintainer needs ongoing guidance and shortcuts.
The GUI should support or at least assist with:

- Version bumping:
  - Track upstream version (if detectable via user‑configured URL or script hook) and suggest new `pkgver`.
  - Allow quick edit of `pkgver` and `pkgrel` and mark these in UI.
- Adopting orphaned packages / disowning:
  - Open the AUR web interface at the relevant pages for adopt/disown actions; full automation is not available via public APIs.[^4]
- Requesting PKGBUILD review prior to submission or for tricky changes, following Arch Wiki recommendations.[^13][^14][^1]
  - Provide direct links to the AUR mailing list, forum sections, or suggested Reddit threads where maintainers commonly request PKGBUILD review.[^14][^13]
- Handling co‑maintainers:
  - Display the maintainer and co‑maintainer list for a package (fetched from AUR API or web) and note that changes affect all users.
- Responding to user feedback:
  - Provide links to comments on the AUR package page.

### GUI Elements

- "Maintenance" tab per package with:
  - Version field summary vs installed version.
  - Buttons to open AUR web page, comments, and adoption/disown forms.
  - "Request review" helper that opens pre‑filled browser windows with package links.

## Cross‑Cutting Concerns

### Logging, Transparency, and Escape Hatches

The GUI must remain transparent and not become a black box:

- Always show the exact commands being run (`makepkg`, `git`, devtools scripts, ssh, etc.) in a side panel or expandable section.
- Provide an option to copy commands so users can run them manually in a terminal.
- Keep per‑package logs for builds and pushes for debugging.

### Error Handling and Recovery

- Parse common failure modes: missing base-devel, devtools not installed, `PKGBUILD` syntax errors, `.SRCINFO` out of date, git identity not configured, AUR not reachable, etc.[^9][^1][^3]
- Provide actionable remediation hints and quick links.
- Support partial completion: if a step fails, allow the user to fix and retry that step without restarting the entire wizard.

### Security Considerations

- Be explicit about where SSH keys are stored and ensure permissions are correct (e.g., `600`).[^2]
- Avoid storing AUR account passwords; rely on the browser for web login.
- Make it clear when the GUI is running commands as root (e.g., `pacman -U`) and use polkit/sudo integration thoughtfully.

### Extensibility and Configurability

Given the variety of packaging workflows and personal preferences:

- Allow advanced users to customise:
  - The exact devtools scripts used.
  - Alternative `.SRCINFO` generation commands.
  - Hooks before/after build or push (e.g., run tests or formatters).
- Abstract backends so the GUI can swap between devtools and tools like `clean-chroot-manager` depending on what is installed.[^8]

## Suggested Architecture for the Standalone GUI

### Core Services

Internally, the GUI can be split into service layers:

- **Environment service**: detects installed packages, configuration files, and system prerequisites.
- **SSH and git service**: manages keys, git remotes, and commit/push operations.
- **Packaging service**: handles PKGBUILD parsing (lightweight), linting heuristics, `.SRCINFO` generation, and links to ArchWiki resources.[^5][^1]
- **Build service**: orchestrates `makepkg` and devtools, capturing logs and artifacts.[^6][^3]
- **AUR API/Web integration service**: queries AUR for package existence, metadata, and opens relevant web pages.[^4]

Each service should expose operations as high‑level methods that the UI can compose into wizards and per‑package views.

### UI Structure

A practical UI layout for a desktop app might include:

- Left sidebar: list of tracked AUR packages and a global "Environment & Identity" section.
- Main area: tabbed view per package covering:
  - Overview (status summary of all stages).
  - PKGBUILD editor.
  - Build (local and chroot sub‑tabs).
  - Git & publish.
  - Maintenance.
- Bottom panel: shared log/console with filters by service (build, git, ssh, chroot).

## Gap Checklist for an Existing Implementation

To use this document to find gaps in an existing GUI implementation, the maintainer can walk through the following checklist:

- Identity & SSH
  - AUR account links present
  - SSH key generation, viewing, and copy‑to‑clipboard support
  - Optional ssh config writer and connectivity tester
- Tooling & Environment
  - base-devel, git, devtools, makepkg detected
  - makepkg.conf and devtools configs discoverable
- Package Bootstrap
  - New package wizard with AUR and official repo duplication checks
  - Existing package clone/import flow
- PKGBUILD Authoring
  - Bash editor with templates and minimal linting
  - Quick links to key ArchWiki pages
- Build & Test
  - Local makepkg integration with log streaming
  - pacman -U install helper for test builds
- Clean Chroot
  - devtools detection and configuration
  - One‑click chroot build and log display
- .SRCINFO
  - makepkg --printsrcinfo integration
  - Sync status indicator and diff view
- Git & Publish
  - Git identity checks, staging, commit, and push flows
  - Standard `.gitignore` helper
- Maintenance
  - Version bump helpers
  - AUR web, comments, and review‑request shortcuts

Each unchecked line suggests a potential feature gap in the GUI that may be worth addressing to fully cover the AUR maintenance lifecycle.

---

## References

1. [AUR submission guidelines - ArchWiki](https://wiki.archlinux.org/title/AUR_submission_guidelines)
2. [Arch Linux Arch User Repository¶](https://wdv4758h-notes.readthedocs.io/zh-tw/latest/archlinux/aur.html)
3. [DeveloperWiki:Building in a clean chroot - ArchWiki](https://wiki.archlinux.org/title/DeveloperWiki:Building_in_a_clean_chroot) - The devtools package provides tools for creating and building within clean chroots. Install it if no...
4. [Arch User Repository - ArchWiki](https://wiki.archlinux.org/title/Arch_User_Repository) - How do I create a PKGBUILD? Consult the AUR submission guidelines#Rules of submission, then see crea...
5. [PKGBUILD - ArchWiki](https://wiki.archlinux.org/title/PKGBUILD) - A PKGBUILD is a Bash script containing the build information required by Arch Linux packages. Packag...
6. [makepkg - ArchWiki](https://wiki.archlinux.org/title/Makepkg) - makepkg is a script to automate the building of packages. The requirements for using the script are ...
7. [Submit a Package to the Arch User Repository](https://dt.iki.fi/submit-package-aur)
8. [AUR (en) - clean-chroot-manager - Arch Linux](https://aur.archlinux.org/packages/clean-chroot-manager) - It's working fine now using my own pacman & makepkg settings, including the ccache data sharing. I e...
9. [[Solved] which makepkg.conf to use for clean chroot building](https://bbs.archlinux.org/viewtopic.php?id=280074) - The tool used to build in clean chroots by devs & TUs is devtools and uses upstream files. Looking a...
10. [I adopted an AUR package - how can I build it in a "clean ... - Reddit](https://www.reddit.com/r/archlinux/comments/9hk33f/i_adopted_an_aur_package_how_can_i_build_it_in_a/) - We build our packages with the makechrootpkg wrapper. You can install it via pacman -S devtools and ...
11. [GitHub - D3vil0p3r/AUR: Arch User Repository packages maintained by D3vil0p3r.](https://github.com/D3vil0p3r/AUR/) - Arch User Repository packages maintained by D3vil0p3r. - D3vil0p3r/AUR
12. [Arch Linux](https://bbs.archlinux.org/viewtopic.php?pid=1914998)
13. [Request - PKGBUILD Review / AUR Issues, Discussion ...](https://bbs.archlinux.org/viewtopic.php?id=270689) - Hello! I'm looking into submitting a PKGBUILD file to AUR and the Arch Wiki suggests I should be ask...
14. [First time publishing PKGBUILD to AUR : r/archlinux - Reddit](https://www.reddit.com/r/archlinux/comments/16xn3l9/first_time_publishing_pkgbuild_to_aur/) - Hey everyone, I read the Arch wiki on guidelines for packaging ... [PKGBUILD Review] Can someone tak...