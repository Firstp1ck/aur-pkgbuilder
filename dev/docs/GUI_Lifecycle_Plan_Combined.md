# Combined GUI lifecycle plan (gap analysis)

This document merges the two internal specifications:

- **Design doc** — `dev/Designing a Standalone GUI to Guide Maintainers Through the Full AUR Package Lifecycle.md` (staged workflow, checklist tone).
- **Compass spec** — `dev/compass_artifact_wf-dcbb91e9-0931-4f4f-8a6e-b801c2f5a1af_text_markdown.md` (command-level detail, AUR server semantics).

Each item is tagged **Done**, **Partial**, or **Missing** against the current `aur-pkgbuilder` tree (GTK + libadwaita, `src/ui/`*, `src/workflow/`*). **Priority** is implementation order for remaining work: **P0** (correctness / push blockers), **P1** (core maintainer loop), **P2** (quality / power user), **P3** (nice-to-have / long tail).

---

## Contradictions reconciled (with sources)


| Topic                      | Design doc                                   | Compass spec                                                        | Resolution                                                                                                                                                                                                                                                                                                                                                                       |
| -------------------------- | -------------------------------------------- | ------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| SSH key algorithm          | RSA 4096 **or** Ed25519, user choice         | Default Ed25519; RSA/ECDSA as fallbacks; warn on weak RSA           | **[ArchWiki — AUR submission guidelines § Authentication](https://wiki.archlinux.org/title/AUR_submission_guidelines)** shows `ssh-keygen -f ~/.ssh/aur` (OpenSSH default is Ed25519 on current Arch). The hard requirement is a **dedicated** keypair for selective revocation, not a specific algorithm. Optional multi-algorithm UI is enhancement, not required by the wiki. |
| `~/.ssh/config` host alias | Example `Host aur`                           | `Host aur.archlinux.org` + `IdentitiesOnly yes`                     | **ArchWiki** documents `Host aur.archlinux.org` with `IdentityFile` and `User aur`. `IdentitiesOnly` is not in the wiki snippet but is standard practice for “too many authentication failures”; keep it. Either host alias works if `git` remote and `ssh` target stay consistent.                                                                                              |
| First-time new package     | Local `git init` + remote                    | **Clone-first** into empty namespace (expected empty-clone warning) | **ArchWiki** explicitly recommends `git -c init.defaultBranch=master clone ssh://aur@aur.archlinux.org/pkgbase.git` and notes the empty-repo warning is normal. Clone-first is canonical; local init is still valid but skips the implicit SSH check.                                                                                                                            |
| Community review vs gate   | “Request review” links before tricky changes | AUR is **unmoderated**; push makes the package public               | **Not a contradiction**: the wiki encourages mailing list / forum review when unsure; that is **voluntary**, not an AUR gate. UX should state clearly that **review does not block publication** — only maintainer discipline does.                                                                                                                                              |
| SSH probe exit code        | “Show result” of SSH test                    | Success string `Welcome…`; exit code may be non-zero                | **Compass + observed aurweb behavior**: treat **banner text**, not exit code, as the signal. The app already looks for `Welcome` in `preflight::probe_aur_ssh`.                                                                                                                                                                                                                  |


**Current tree (UX vs this table):** Rows on **first-time clone**, **community review vs gate**, and **publication immediacy** are reflected in copy on **Publish** (`ui/publish.rs` — intro, “Publication expectations” group, tooltips/toasts), with shorter non-duplicative echoes on **Sync** (`ui/sync.rs` — “Sync and Publish” group) and **Add package** (`ui/package_editor.rs` — group description for new entries only, plus **C4**/**C5** pkgbase hint + validation + namespace probe on **Save**). A **standalone first-pkgbase wizard** is still not present; see **C3**.


---

## Cross-cutting (both documents)


| #   | Statement (merged)                                                                                       | Status                                                                                                   | Priority |
| --- | -------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------- | -------- |
| C1  | Transparent logging: show exact commands (`makepkg`, `git`, `ssh`, …)                                    | **Partial** — streamed `$ …` lines in log views; no persistent per-package log index                     | P2       |
| C2  | Copy-to-clipboard for commands / escape hatch to run manually                                            | **Missing**                                                                                              | P2       |
| C3  | Actionable errors + retry failed step without full wizard reset                                          | **Partial** — toasts + re-run buttons; no structured “recovery modal”                                    | P1       |
| C4  | Security: SSH key paths + permissions; do not store AUR passwords; surface privilege for root operations | **Partial** — permissions enforced on SSH writes; `makepkg` root refusal; no polkit flow for `pacman -U` | P1       |
| C5  | Configurable backends (devtools vs wrappers, hooks)                                                      | **Missing**                                                                                              | P3       |
| C6  | Left nav: global environment + per-package workflow                                                      | **Partial** — tabbed shell + home list; not a full split-view dashboard                                  | P2       |
| C7  | Shared bottom log with filters by subsystem                                                              | **Missing** — single log style per screen                                                                | P3       |


---

## Stage A — Identity, SSH, account


| #   | Statement                                                                                                                       | Status                                                                                         | Priority |
| --- | ------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------- | -------- |
| A1  | Link / open AUR account; registration is manual (browser)                                                                       | **Done** — With username: `xdg-open` on `https://aur.archlinux.org/account/<user>/edit` via `ssh_setup::open_aur_account_page` / `aur_account_edit_url`. **Without** username: `ui/ssh_setup` shows `adw::Dialog` (padded title/body + `EntryRow`; **Cancel** closes; **Continue** → `open_aur_register_page` / `AUR_REGISTER_URL`; **Save and open** runs `aur_account::apply_aur_username_with_registry_check` like Connection, then opens account URL). | —        |
| A2  | Generate **dedicated** AUR key; **never** overwrite existing private key                                                        | **Done** — `ensure_aur_key`                                                                    | —        |
| A3  | Show public key; copy to clipboard                                                                                              | **Done** — `ui/ssh_setup`                                                                      | —        |
| A4  | Optional `~/.ssh/config` for AUR host with `IdentityFile`                                                                       | **Done** — `write_ssh_config_entry`                                                            | —        |
| A5  | `IdentitiesOnly yes` to avoid “too many authentication failures”                                                                | **Done** — in rendered block                                                                   | —        |
| A6  | List **all** existing keys under `~/.ssh` with fingerprints (`ssh-keygen -lf`)                                                  | **Done** — `list_keys` + per-`.pub` `ssh-keygen -lf` SHA256 on each row in `ui/ssh_setup`       | —        |
| A7  | SSH connectivity test; parse **Welcome** / key errors                                                                           | **Done** — `preflight::probe_aur_ssh`                                                          | —        |
| A8  | Host key handling: verify fingerprints against **published** AUR keys (Ed25519 / ECDSA / RSA SHA256), refresh path vs hard-code | **Done** — `ssh-keyscan` lines verified against HTTPS scrape of `AUR_WEB_HOMEPAGE` (3–10 plausible `SHA256:` tokens) else bundled fallback; refuse `known_hosts` append on mismatch | —        |
| A9  | Normalize clipboard pubkey (single line, trim) to reduce “invalid key” paste errors                                             | **Done** — `normalize_pubkey_for_clipboard` + copy path in `ui/ssh_setup`                      | —        |
| A10 | Passphrase recommendation + `ssh-add` / agent verification step                                                                 | **Partial** — UI copy + “Check agent” / `ssh-add` actions; `ensure_aur_key` still uses `-N ""` | P1       |
| A11 | Note: multiple keys in AUR profile = newline-separated; one pubkey → one account                                                | **Done** — Publish group description in `ui/ssh_setup`                                         | —        |
| A12 | Note: new AUR accounts may require manual approval (spam); set expectations in wizard                                           | **Done** — onboarding Login group description in `ui/onboarding.rs`                            | —        |
| A13 | View/change **AUR username** on Connection; apply saves only after RPC check; Home **red** rows for registry ids not in maintainer∪co-maint RPC set | **Done** — `ui/connection.rs` (EntryRow apply), `aur_account::{apply_aur_username_with_registry_check, …}`, `state::AppState::aur_account_mismatch_ids`, `ui/home.rs`, `MainShell::refresh_home_list`. **Also:** Connection username row is registered on `MainShell` and refreshed when the username is saved from the SSH-setup missing-user dialog or onboarding fetch (`register_connection_aur_username_row` / `refresh_connection_aur_username_field` in `ui/shell.rs`). | —        |


---

## Stage B — Tooling and environment

**Scope:** everything the maintainer needs on the host *before* touching a pkgbase — core CLI tools on `PATH`, completeness of the build toolchain (`base-devel`), checksum helpers, and (later) clean-chroot entrypoints plus discoverable config files. **Connection** shows **Required tools** (`preflight::check_tools`), **Recommended environment** (one async fill: `preflight::check_environment_recommended` → `check_base_devel_group`, `check_fakeroot_sentinel`, `check_devtools_bundle`; `preflight::ToolCheck` uses `satisfied_without_binary` + `detail` for the `pacman -Qg` row), **Packaging configuration** (`gtk4::FileLauncher` on the main thread in `ui/connection` + `preflight::packaging_config_path` / `PackagingConfigTarget`), and **Validate** (`workflow/validate.rs`) for deeper optional steps (e.g. fakeroot build).

| #   | Statement                                                             | Status                                                                                                                                                                                                                                                                                                                                                         | Priority |
| --- | --------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------- |
| B1  | Detect `makepkg`, `git`, `ssh`                                        | **Done** — `preflight::check_tools` (`which` per program; list lives in `preflight.rs`); rows from `ui/connection::render_tool_row` (subtitle = purpose, missing row shows `install_hint`).                                                                                                                                                                      | —        |
| B2  | Detect / require `base-devel` effectively (not only `makepkg` binary) | **Done** — `preflight::check_base_devel_group` runs `pacman -Qg base-devel` (non-empty ⇒ pass) plus `check_fakeroot_sentinel` as a second signal. Does **not** diff against the full sync-db group list (only “has members installed”).                                                                                                                        | —        |
| B3  | Detect `updpkgsums` (`pacman-contrib`)                                | **Done** — same preflight table as B1; execution path `workflow/build::run_updpkgsums` + Version tab wiring (`ui/version.rs`).                                                                                                                                                                                                                                 | —        |
| B4  | Detect `devtools` / document clean-chroot path                        | **Partial** — `preflight::check_devtools_bundle` treats first hit among `pkgctl`, `extra-x86_64-build`, `makechrootpkg` as satisfied; Connection row + wiki blurb in group description. Overlaps **G1** detection; **still missing:** chroot matrix path, `pkgctl build` wiring, and the fuller narrative planned for **G3**.                                  | **P1**   |
| B5  | Optional: `pkgbuild-introspection` / `mksrcinfo`                      | **Missing** — app uses `makepkg --printsrcinfo` only (aligned with modern practice)                                                                                                                                                                                                                                                                            | P3       |
| B6  | Open `makepkg.conf` / devtools conf snippets in preferred editor      | **Partial** — Connection **Packaging configuration** rows use `gtk4::FileLauncher` (main thread) with allowlisted paths from `preflight::packaging_config_path`. **Still missing:** per-snippet rows (e.g. individual `makepkg-*.conf` under `pacman.conf.d`).                                                                                                    | P3       |
| B7  | Explain why chroot + `base-devel` matter                              | **Partial** — **Recommended environment** copy names `base-devel`, `fakeroot`, `devtools`, typical `/var/lib/archbuild`, and the clean-chroot wiki URL; **Packaging configuration** copy points at GTK + default app for `.conf` / folders. **Still missing:** dedicated tips panel / disk + sudo expectations (**G3**) once chroot builds ship.                    | P2       |


---

## Stage C — Bootstrap packages (registry / clone / new)


| #   | Statement                                                                           | Status                                                                                         | Priority |
| --- | ----------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------- | -------- |
| C1  | Import packages user maintains (RPC by maintainer / co-maintainer)                  | **Done** — `onboarding` + `aur_account::fetch_my_packages`                                     | —        |
| C2  | Per-package working directory + safe path validation                                | **Done** — `sync::package_dir`, folder pick                                                    | —        |
| C3  | **Clone-first** new pkgbase; treat empty clone warning as informational             | **Partial** — `aur_git::ensure_clone` + Publish/Sync/Add-package copy (empty clone + push is public); **no** dedicated “new empty namespace” wizard | P2       |
| C4  | Enforce **pkgbase** naming regex; distinguish pkgbase vs pkgname for split packages | **Done** — `workflow/pkgbase::validate_aur_pkgbase_id` (ASCII `[a-z0-9@._+-]+`); **Add package** row title + hint label for pkgbase vs split `pkgname`; `PackageDef::id` + `registry` header docs. **Still missing:** PKGBUILD-level split warnings (**D7**). | —        |
| C5  | Check AUR + official repos for duplicates before first push                         | **Partial** — on **Add package** → **Save** for **new** entries: `workflow/pkgbase::check_pkgbase_publish_namespace` (`aur_account::aur_pkgbase_exists` with `PackageBase` on RPC `type=info`, plus `pacman -Si`); official hit blocks; AUR hit confirm dialog. **Still missing:** same probe as an explicit **Publish** / pre-push gate. | P1       |
| C6  | New-package PKGBUILD templates (Rust/Python/Go/VCS `-git` / `-bin` archetypes)      | **Missing** — editor starts from synced URL content, not templated wizard                      | P1       |
| C7  | “Monorepo / aurpublish-style” advanced layout                                       | **Missing**                                                                                    | P3       |


---

## Stage D — PKGBUILD authoring


| #   | Statement                                                                                        | Status                                        | Priority |
| --- | ------------------------------------------------------------------------------------------------ | --------------------------------------------- | -------- |
| D1  | Bash/PKGBUILD-aware editor (highlighting)                                                                 | **Missing** — `TextView` buffer               | P2       |
| D2  | Snippet / template insertion                                                                     | **Missing**                                   | P2       |
| D3  | Structured form + raw PKGBUILD dual view                                                         | **Partial** — quick metadata rows + full text | P1       |
| D4  | Field / guideline lint: SPDX licenses (`RFC 16` style), legacy `GPL` warnings, “convert to SPDX” | **Missing**                                   | P2       |
| D5  | REUSE / `pkgctl license` setup & check                                                           | **Missing**                                   | P3       |
| D6  | VCS `source` table (fragments, `git+`, signed)                                                   | **Missing**                                   | P2       |
| D7  | Split-package warnings (global `depends` vs per-split for `--syncdeps`)                          | **Missing**                                   | P2       |
| D8  | Dependency row with pacman + **AUR RPC** “found / orphaned / provides”                           | **Missing**                                   | P1       |
| D9  | Quick links to ArchWiki (`PKGBUILD`, `makepkg`, AUR guidelines)                                  | **Missing**                                   | P3       |
| D10 | Safe metadata parse: use `makepkg --printsrcinfo`, never `source` PKGBUILD                       | **Done** — validation + generation paths      | —        |


---

## Stage E — Checksums


| #   | Statement                                              | Status                                                                                                         | Priority |
| --- | ------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------- | -------- |
| E1  | `updpkgsums` as primary regen; diff before/after       | **Partial** — runs `updpkgsums`; smart no-churn restore in `workflow/build`; **no** dedicated checksum diff UI | P2       |
| E2  | Flag stale checksums when `source` edits without regen | **Missing**                                                                                                    | P2       |


---

## Stage F — Local build & install test


| #   | Statement                                                                           | Status                                                 | Priority |
| --- | ----------------------------------------------------------------------------------- | ------------------------------------------------------ | -------- |
| F1  | Stream `makepkg` output                                                             | **Done**                                               | —        |
| F2  | Expose common flags (`-s`, `-i`, `-r`, `-c`, `-C`, `-e`, …) / per-package overrides | **Partial** — `--nobuild`, `--clean` only; always `-f` | P1       |
| F3  | Surface `PACKAGER` / `makepkg.conf` / env overrides                                 | **Missing**                                            | P2       |
| F4  | List / pick built `*.pkg.tar.`*                                                     | **Missing**                                            | P2       |
| F5  | Install test via `pacman -U` with elevation UX                                      | **Missing**                                            | P1       |
| F6  | PTY / VTE for color + carriage returns                                              | **Missing** — pipe/read line log                       | P2       |
| F7  | Parse `makepkg` stages / exit codes into user-facing categories                     | **Missing**                                            | P2       |


---

## Stage G — Clean chroot & release-quality checks


| #   | Statement                                                              | Status                                                       | Priority |
| --- | ---------------------------------------------------------------------- | ------------------------------------------------------------ | -------- |
| G1  | `devtools` / `makechrootpkg` / `extra-x86_64-build` detection & config | **Partial** — same binary probe as **B4** (`preflight::check_devtools_bundle` + Connection row); **still missing:** chroot matrix path, `pkgctl build` / `makechrootpkg` wiring, and config UI beyond opening `/usr/share/devtools`. | **P1**   |
| G2  | Primary action: `pkgctl build` with `--checkpkg --namcap` (Compass)    | **Missing**                                                  | **P1**   |
| G3  | Document disk / sudo / btrfs snapshot expectations                     | **Missing**                                                  | P2       |
| G4  | `namcap` PKGBUILD + built package with severity UI + `-m` tag mapping  | **Partial** — optional `namcap` in validate; basic streaming | P2       |
| G5  | `shellcheck` integrated with PKGBUILD-appropriate suppressions         | **Done** — `workflow/validate`                               | —        |
| G6  | `checkpkg` / soname diff panel                                         | **Missing**                                                  | P2       |
| G7  | Container smoke test (podman/docker minimal install)                   | **Missing**                                                  | P3       |


---

## Stage H — `.SRCINFO`


| #   | Statement                                                                                      | Status                                                                                                                     | Priority |
| --- | ---------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------- | -------- |
| H1  | Regenerate via `makepkg --printsrcinfo`                                                        | **Done**                                                                                                                   | —        |
| H2  | Block / warn when generated output ≠ committed `.SRCINFO`                                      | **Partial** — validate runs printsrcinfo check on working tree; **no** always-on banner vs last committed in clone         | **P0**   |
| H3  | Dedicated `.SRCINFO` diff view before commit                                                   | **Partial** — publish shows `git diff` after staging (includes `.SRCINFO` if changed); **no** split-pane SRCINFO-only diff | P1       |
| H4  | Pre-commit hook to regen `.SRCINFO` when PKGBUILD staged                                       | **Missing**                                                                                                                | P2       |
| H5  | Remind: **every** commit in a push range must carry valid `.SRCINFO` (server hook walks range) | **Missing** — UX copy + amend guidance                                                                                     | **P0**   |


---

## Stage I — Git publish


| #   | Statement                                                                                               | Status                                                                    | Priority |
| --- | ------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------- | -------- |
| I1  | Clone / reuse AUR remote under work dir                                                                 | **Done** — `aur_git::ensure_clone`                                        | —        |
| I2  | Stage `PKGBUILD` + `.SRCINFO`, commit, push                                                             | **Done** — `commit_and_push`                                              | —        |
| I3  | Commit message templates                                                                                | **Done** — config template + per-commit edit                              | —        |
| I4  | `**master` branch only** — detect `main`, offer `git branch -m master`                                  | **Missing** — pushes `origin HEAD` without branch audit                   | **P0**   |
| I5  | Warn / configure `git user.name` / `user.email`                                                         | **Missing**                                                               | P1       |
| I6  | Stage **all** referenced files (patches, `.install`, units, licenses) — not only PKGBUILD/.SRCINFO      | **Missing** — `stage_files` copies only two files from build dir to clone | **P0**   |
| I7  | `.gitignore` helper (whitelist or denylist)                                                             | **Missing**                                                               | P2       |
| I8  | Forbid / explain **no force-push** + non-fast-forward server behavior                                   | **Missing** explicit guardrail UX                                         | P1       |
| I9  | Map `remote: error:` hook strings → remedies (blob size, URL length, missing install, wrong pkgbase, …) | **Missing**                                                               | **P0**   |
| I10 | Pre-push review pane: `git log --stat origin/master..HEAD`, combined diff, lint summary                 | **Partial** — diff + streaming log on publish; **no** `log --stat` slice  | P1       |
| I11 | Open package page after successful push + RPC freshness check                                           | **Missing**                                                               | P2       |


---

## Stage J — Maintenance & long-term


| #   | Statement                                                                  | Status                                                                  | Priority |
| --- | -------------------------------------------------------------------------- | ----------------------------------------------------------------------- | -------- |
| J1  | Version bump helpers (`pkgver` reset `pkgrel`, pkgrel-only, guarded epoch) | **Partial** — kind hints + manual fields; **no** one-click bump actions | P2       |
| J2  | `nvchecker` / `nvcmp` / `nvtake` integration + notifications               | **Missing** — Manage tab stubs (`AdminError::NotImplemented`)           | P2       |
| J3  | Browser shortcuts: package page, comments, flag, adopt/disown, requests    | **Missing**                                                             | P2       |
| J4  | Co-maintainer / orphan / OOD metadata on dashboard columns                 | **Partial** — RPC data exists; **no** rich dashboard                    | P2       |
| J5  | Batch: validate all / build selected / push selected                       | **Missing**                                                             | P3       |
| J6  | Cache `packages-meta-ext-v1.json.gz` + rate-limit aware RPC                | **Missing**                                                             | P3       |


---

## Suggested implementation priority (summary)

1. **P0 — Push correctness**
  Enforce **AUR `master` branch** (ArchWiki), **staging for all tracked files**, `**.SRCINFO` drift vs hook expectations**, **pre-push hook error parsing**. **Pkgbase charset + bootstrap namespace checks** for new registry rows are **implemented** (`workflow/pkgbase`, `aur_account::aur_pkgbase_exists`, **Add package** in `ui/package_editor.rs`); optional **Publish**-time re-probe remains (**C5**). **Host-key verification** against published fingerprints is **implemented** (`workflow/ssh_setup` + homepage/fallback list); remaining P0 items are branch/staging/SRCINFO/push-parse as above.
2. **P1 — Maintainer-quality loop**
  Clean chroot **actions** (`pkgctl build` / `makechrootpkg` UI, matrix path) on top of Connection **devtools detection** (**B4** / **G1**), richer **makepkg** flags, **pacman -U** test path, **dependency+AUR lookup** in editor, optional **first-pkgbase wizard** (copy for clone-first / empty namespace already on Publish/Sync/Add package), **git identity** warnings.
3. **P2 — Professional polish**
  SPDX/legacy license lint, SRCINFO-focused diff, checksum diff, PTY log, `checkpkg`, dashboard columns, `nvchecker`, extra packaging-path shortcuts beyond Connection’s `gtk4::FileLauncher` rows, recovery modals.
4. **P3 — Ecosystem & power users**
  REUSE tooling, monorepo mode, batch ops, cached meta dump, container smoke tests, command clipboard, log filters.

---

## Traceability

- **ArchWiki** primary references used above: [AUR submission guidelines](https://wiki.archlinux.org/title/AUR_submission_guidelines), [Arch User Repository](https://wiki.archlinux.org/title/Arch_User_Repository), [DeveloperWiki:Building in a clean chroot](https://wiki.archlinux.org/title/DeveloperWiki:Building_in_a_clean_chroot) (cited in design doc for devtools role).
- **Implemented behavior** was cross-checked against: `src/workflow/ssh_setup.rs`, `aur_account.rs`, `pkgbase.rs`, `preflight.rs`, `aur_git.rs`, `build.rs` (UI + workflow), `validate.rs`, `publish.rs`, `sync.rs`, `package_editor.rs`, `pkgbuild_editor.rs`, `onboarding.rs`, `manage.rs`, `state.rs`, `ui/connection.rs`, `ui/version.rs`, `ui/home.rs`, `ui/shell.rs`, `ui/ssh_setup.rs`.

When this plan changes, update the **Status** column in place rather than forking another prose spec — keep one living backlog file.