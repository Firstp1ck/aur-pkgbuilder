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
| A1  | Link / open AUR account; registration is manual (browser)                                                                       | **Done** — `xdg-open` AUR account URL                                                          | —        |
| A2  | Generate **dedicated** AUR key; **never** overwrite existing private key                                                        | **Done** — `ensure_aur_key`                                                                    | —        |
| A3  | Show public key; copy to clipboard                                                                                              | **Done** — `ui/ssh_setup`                                                                      | —        |
| A4  | Optional `~/.ssh/config` for AUR host with `IdentityFile`                                                                       | **Done** — `write_ssh_config_entry`                                                            | —        |
| A5  | `IdentitiesOnly yes` to avoid “too many authentication failures”                                                                | **Done** — in rendered block                                                                   | —        |
| A6  | List **all** existing keys under `~/.ssh` with fingerprints (`ssh-keygen -lf`)                                                  | **Partial** — `list_keys` exists; UI depth varies                                              | P2       |
| A7  | SSH connectivity test; parse **Welcome** / key errors                                                                           | **Done** — `preflight::probe_aur_ssh`                                                          | —        |
| A8  | Host key handling: verify fingerprints against **published** AUR keys (Ed25519 / ECDSA / RSA SHA256), refresh path vs hard-code | **Missing** — `ssh-keyscan` + append + show fingerprints; **no** comparison to known-good list | **P0**   |
| A9  | Normalize clipboard pubkey (single line, trim) to reduce “invalid key” paste errors                                             | **Missing**                                                                                    | P2       |
| A10 | Passphrase recommendation + `ssh-add` / agent verification step                                                                 | **Missing** — keys generated with empty passphrase `-N ""` today                               | P1       |
| A11 | Note: multiple keys in AUR profile = newline-separated; one pubkey → one account                                                | **Partial** — not all copy UX copydeck                                                         | P3       |
| A12 | Note: new AUR accounts may require manual approval (spam); set expectations in wizard                                           | **Missing**                                                                                    | P2       |


---

## Stage B — Tooling and environment


| #   | Statement                                                             | Status                                                                              | Priority |
| --- | --------------------------------------------------------------------- | ----------------------------------------------------------------------------------- | -------- |
| B1  | Detect `makepkg`, `git`, `ssh`                                        | **Done** — `preflight::check_tools`                                                 | —        |
| B2  | Detect / require `base-devel` effectively (not only `makepkg` binary) | **Missing** — install hint points at `base-devel` but no group probe                | P2       |
| B3  | Detect `updpkgsums` (`pacman-contrib`)                                | **Done**                                                                            | —        |
| B4  | Detect `devtools` / document clean-chroot path                        | **Missing**                                                                         | **P1**   |
| B5  | Optional: `pkgbuild-introspection` / `mksrcinfo`                      | **Missing** — app uses `makepkg --printsrcinfo` only (aligned with modern practice) | P3       |
| B6  | Open `makepkg.conf` / devtools conf snippets in preferred editor      | **Missing**                                                                         | P3       |
| B7  | Explain why chroot + `base-devel` matter                              | **Partial** — scattered hints; no dedicated tips panel                              | P2       |


---

## Stage C — Bootstrap packages (registry / clone / new)


| #   | Statement                                                                           | Status                                                                                         | Priority |
| --- | ----------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------- | -------- |
| C1  | Import packages user maintains (RPC by maintainer / co-maintainer)                  | **Done** — `onboarding` + `aur_account::fetch_my_packages`                                     | —        |
| C2  | Per-package working directory + safe path validation                                | **Done** — `sync::package_dir`, folder pick                                                    | —        |
| C3  | **Clone-first** new pkgbase; treat empty clone warning as informational             | **Partial** — publish path clones AUR repo; **no** dedicated “new empty namespace” wizard copy | P1       |
| C4  | Enforce **pkgbase** naming regex; distinguish pkgbase vs pkgname for split packages | **Missing** in UI validation                                                                   | **P0**   |
| C5  | Check AUR + official repos for duplicates before first push                         | **Missing**                                                                                    | **P0**   |
| C6  | New-package PKGBUILD templates (Rust/Python/Go/VCS `-git` / `-bin` archetypes)      | **Missing** — editor starts from synced URL content, not templated wizard                      | P1       |
| C7  | “Monorepo / aurpublish-style” advanced layout                                       | **Missing**                                                                                    | P3       |


---

## Stage D — PKGBUILD authoring


| #   | Statement                                                                                        | Status                                        | Priority |
| --- | ------------------------------------------------------------------------------------------------ | --------------------------------------------- | -------- |
| D1  | Bash-aware editor (highlighting)                                                                 | **Missing** — `TextView` buffer               | P2       |
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
| G1  | `devtools` / `makechrootpkg` / `extra-x86_64-build` detection & config | **Missing**                                                  | **P1**   |
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
  Enforce **AUR `master` branch** (ArchWiki), **pkgbase validation**, **staging for all tracked files**, `**.SRCINFO` drift vs hook expectations**, **pre-push hook error parsing**, **host-key verification** against published fingerprints.
2. **P1 — Maintainer-quality loop**
  Clean chroot (`pkgctl build` / devtools), richer **makepkg** flags, **pacman -U** test path, **dependency+AUR lookup** in editor, **clone-first / empty namespace** onboarding copy, **git identity** warnings.
3. **P2 — Professional polish**
  SPDX/legacy license lint, SRCINFO-focused diff, checksum diff, PTY log, `checkpkg`, dashboard columns, `nvchecker`, xdg-open maintenance links, recovery modals.
4. **P3 — Ecosystem & power users**
  REUSE tooling, monorepo mode, batch ops, cached meta dump, container smoke tests, command clipboard, log filters.

---

## Traceability

- **ArchWiki** primary references used above: [AUR submission guidelines](https://wiki.archlinux.org/title/AUR_submission_guidelines), [Arch User Repository](https://wiki.archlinux.org/title/Arch_User_Repository), [DeveloperWiki:Building in a clean chroot](https://wiki.archlinux.org/title/DeveloperWiki:Building_in_a_clean_chroot) (cited in design doc for devtools role).
- **Implemented behavior** was cross-checked against: `src/workflow/ssh_setup.rs`, `preflight.rs`, `aur_git.rs`, `build.rs` (UI + workflow), `validate.rs`, `publish.rs`, `pkgbuild_editor.rs`, `onboarding.rs`, `manage.rs`.

When this plan changes, update the **Status** column in place rather than forking another prose spec — keep one living backlog file.