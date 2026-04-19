# Plan: Register / create a new AUR package (first push)

This document tracks **“Register new AUR package”** end-to-end: a safe, observable workflow aligned with Arch AUR expectations and existing app patterns. **MVP status (2026-04):** the `workflow::admin::register_on_aur` stub is **replaced**; Home opens a **Register wizard**; backend gates and streaming logs are **implemented** — see [Implementation status](#implementation-status) for gaps vs this document’s full intent.

**Related code (current)**

- **UI entry:** **Home** — `src/ui/home.rs` (“Register new AUR package”) pushes `src/ui/register.rs` (**Register wizard**): define package via `package_editor` → `Registry::upsert` + `save` → `runtime::spawn_streaming` → `admin::register_on_aur(work_dir, pkg, tx, remote_history_mode)` with **LogView**. Does **not** use `state.package`. `manage::start_register_new_aur_package` was **removed** (Manage tab never owned Register).
- **Orchestration:** `src/workflow/admin.rs` — `register_on_aur`, `RegisterRemoteHistoryMode`, typed `AdminError` (namespace, root, remote history, validation, …). Rustdoc: **clone-first**; wiki **init + remote + fetch** = manual-only.
- **Git helpers:** `src/workflow/aur_git.rs` — `ensure_clone` (`-c init.defaultBranch=master` + `ensure_named_master_branch` on **new** clones only), `ls_remote_has_any_ref`, `origin_master_resolves`, `log_origin_master_oneline`, `fetch_origin`, existing `stage_files` / `commit_and_push`. Unit tests: ls-remote empty vs populated bare, branch rename (`target/aur_git_test_*`).
- **Root guard (shared):** `src/workflow/privilege.rs` — `nix_is_root()`; **Build** tab uses the same helper (`src/ui/build.rs`).
- **Publish** (unchanged vs Register): `src/ui/publish.rs` — Prepare still **does not** run `validate::run_all` (P1: shared pre-stage gate).
- **Validate pipeline:** `src/workflow/validate.rs` — Register calls **`run_all`** + **`required_tier_all_pass`** before clone/stage.
- **Pkgbase namespace:** `src/workflow/pkgbase.rs` — Register calls **`check_pkgbase_publish_namespace`** early (same semantics as `package_editor` for official / AUR hits).
- Lifecycle context: `dev/docs/GUI_Lifecycle_Plan_Combined.md` (clone-first vs local `git init`).

**Authoritative external reference**

- [AUR submission guidelines](https://wiki.archlinux.org/title/AUR_submission_guidelines) — account requirements, first-time `git clone` over SSH, empty-repository warning, push creates the remote package.

**Publish vs this plan (important)**

- **Publish “Prepare” today** runs: `ensure_clone` → `build_wf::write_srcinfo` → `stage_files` → `diff`. It does **not** run `workflow::validate` or `makepkg --nobuild`.
- **Register** should **not** inherit that gap: before the first commit, run the **implemented** validation pipeline below. When touching Publish, consider sharing the same pre-stage gate so both flows stay consistent.

---

## Goals

1. Let a maintainer perform the **first** publication of a **new** pkgbase to `ssh://aur@aur.archlinux.org/<pkgbase>.git` without leaving the app, starting from the **Home** tab **Register new AUR package** control. The flow supplies its own target: a **`PackageDef` + PKGBUILD tree** created or confirmed inside the Register wizard (see [Register entry point](#register-entry-point-not-home-selection)), not merely whatever package is currently selected for Sync/Build/Publish.
2. **Fail closed** on a **naive** first-import when the remote already has history; **clone** so **`git log`** can run, then require an explicit user choice to **continue on history** or **abort**, after **warning** and **showing** bounded remote history — never silent overwrite. **MVP:** strict mode aborts after log; “continue” mode runs **`git fetch origin`**, verifies `origin` URL, then stages/commits — **full rebase/merge when the local clone has diverged** is still [outstanding](#implementation-status).
3. Reuse **security and UX rules** from the rest of the tree: `Command::new().arg()` only (no shell interpolation of paths or package names), SSH verification before network git, streamed diagnostics where appropriate, clear actionable errors.
4. Before any **first** commit to the AUR clone, run the **same automated checks** the app already implements in `workflow::validate` (see [Pre-commit quality gate](#pre-commit-quality-gate-implemented-checks)).
5. **Official repositories:** if the pkgbase name already exists in the maintainer’s **sync databases** (`core` / `extra` / `community` / … as configured), **block** Register — matches [AUR submission guidelines](https://wiki.archlinux.org/title/AUR_submission_guidelines) (“must not build applications already in any of the official binary repositories”). Reuse **`pkgbase::check_pkgbase_publish_namespace`** (`official_repo_hit`).

## Non-goals (explicit)

- Automating AUR **web** account signup or Trusted User (TU) actions.
- “Import from existing AUR” / upstream check / archive (`admin.rs` siblings) — out of scope except where shared helpers naturally land.
- Re-implementing checks that already live under `workflow::validate` — **call** that module instead of duplicating `makepkg`/`bash` invocations.
- Automating every remaining **policy** rule on the wiki (AUR duplicate adoption UX, naming heuristics, maintainer header text, usefulness) — see [Wiki expectations not fully automated](#wiki-expectations-not-fully-automated); optional future hints only. **Exception:** **official-repo name collision** is **in scope** for Register via `pkgbase::check_pkgbase_publish_namespace` (`pacman -Si`), not hand-waved to “maintainer only.”

---

## Register entry point (not home selection)

**Problem:** “Register new AUR package” names a **greenfield** action. Binding it to `state.package` (whatever package is currently selected for the workflow) is easy to wire but wrong: the user may have another package open for daily work while they intend to **create** a new AUR repo.

**Current UI (MVP)** — **Home → Register new AUR package** pushes a **navigation page** (`ui/register.rs`): SSH banner when `!ssh_ok`, **Define package…** (full `package_editor`), checkbox **“Allow existing remote Git history”** (`RegisterRemoteHistoryMode::AllowExistingRemoteHistory` vs strict), **Validate, clone, and push to AUR** + **LogView**.

**Product rule**

- **Register new AUR package** is a **dedicated** flow for the **new** pkgbase (first push); it does **not** use implicit `state.package` selection.
- **Publish** (and related per-package tabs) continue to use the **currently selected** package — that is the normal “update existing AUR package” path.

**Implementation direction**

1. ~~**Home → Register wizard**~~ **Done** — `home.rs` → `register::build` → `register_on_aur(pkg from wizard, …)`.
2. **Wizard steps (MVP coverage):**
   - **Identity + paths + PKGBUILD:** covered by opening **Add/edit package** (`package_editor`) from the wizard — same `PackageDef` fields, namespace on save, `sync::package_dir` once work dir + destination are set. **Not done:** dedicated in-wizard steps / minimal PKGBUILD template without opening the full editor.
   - **On push:** **`Registry::upsert` + `save`** happen when the user saves from the editor; **`register_on_aur`** uses that **`PackageDef`**.
3. **Optional (still open):** after success, **“Open this package on home”** (set `state.package` + refresh).

---

## Pre-commit quality gate (implemented checks)

**Answer:** The earlier draft table mentioned `makepkg --nobuild` (aligned with `dev/scripts/aur-push.sh`) but **did not** spell out the **Validate** pipeline. **Register must** integrate the checks that already exist in code.

**Namespace gate ordering**

- Run **`check_pkgbase_publish_namespace`** **early** (before `validate::run_all` / clone) so the maintainer does not spend time on PKGBUILD checks when the name is **disallowed** (official hit) or **already an AUR pkgbase** (index hit). Reuses the same helper as the package id field in **`package_editor.rs`**.
- **`pacman -Si`** reflects whatever is in the user’s **synced** official databases (`pacman -Sy` / mirror freshness); it is the project’s chosen probe — not a separate HTTP scrape of `archlinux.org/packages` in MVP.

| Gate | Source | Blocking? |
| ---- | ------ | --------- |
| **Official repos + AUR index** | `pkgbase::check_pkgbase_publish_namespace(trimmed_pkg_id)` — `pacman -Si` against **sync** DBs + AUR RPC (`aur_account::aur_pkgbase_exists`) | **Yes** — **abort** if `official_repo_hit` (wiki: must not duplicate official binary packages). **Abort** greenfield Register if `aur_pkgbase_hit` (pkgbase already on the AUR; use **Publish** / adoption, not “new package” register). On `PkgbaseNsError::Pacman`, abort with actionable text (e.g. sync databases, `pacman` on `PATH`). |
| **Required tier** | `validate::run_tier(CheckTier::Required, …)` — `bash -n PKGBUILD`, `makepkg --printsrcinfo`, `makepkg --verifysource` | **Yes** — abort register if any outcome is not `Pass` (use `validate::required_tier_all_pass` on the required reports, or equivalent logic). |
| **Optional tier** | `validate::run_tier(CheckTier::Optional, …)` — `shellcheck`, `namcap` on PKGBUILD | **No** — same semantics as the Validate tab: missing tools → `Skipped`; warnings → `Warn`, do not block push. |
| **Convenience** | `validate::run_all` runs **required + optional** in one call (it **excludes** extended — see `validate.rs`). | Prefer **`run_all`** for Register’s default gate so one code path matches “everything fast the app knows how to run.” |

**Extended tier** (`run_extended`: fakeroot full build + `namcap` on the package) is **intentionally not** part of the default Register gate — it can take a long time. Users can still run it from the **Validate** tab before Register if they want.

**Ordering with `.SRCINFO` on disk**

- `run_all` already includes a **printsrcinfo** check (success/failure) but **Publish** persists via `build_wf::write_srcinfo`. After `run_all` succeeds, still run **`write_srcinfo`** so `.SRCINFO` on disk matches what you stage (today’s Publish ordering). A later optimization could avoid a second `makepkg --printsrcinfo` if reports are refactored to expose captured output — not required for MVP.

**`makepkg --nobuild`**

- Not a separate check in `validate.rs`. The shell script `aur-push.sh` uses it as a coarse “prepare” step. If product wants **script parity** in addition to `run_all`, add an explicit `build_wf::run_makepkg(…, &["--nobuild"], …)` step and treat failure like a hard gate — document as optional beyond `run_all`.

**Root**

- Do not run `makepkg` as root — **`workflow::privilege::nix_is_root()`** (shared with **Build** in `ui/build.rs`); Register refuses when root before `validate::run_all`.

---

## Wiki expectations not fully automated

These items appear on [AUR submission guidelines](https://wiki.archlinux.org/title/AUR_submission_guidelines); the app cannot fully enforce them in Register MVP, but the plan should not pretend they do not exist.

1. **Extra tracked files** — Wiki: commit patches, `.install` files, vendored sources under the package git tree as needed. **Today** `aur_git::stage_files` and `commit_and_push` only handle **`PKGBUILD`** and **`.SRCINFO`**. That is enough for minimal packages with no local sources; packages with extra files need **extended staging** (e.g. copy or `git add` all intended files from the Sync directory into the clone, aligned with Publish once both support it).
2. **Package source license** — Wiki encourages `LICENSE` / `REUSE.toml` and licensing guidance for promotion to official repos. Register does not need to generate these files; optionally surface a reminder in UI or docs when absent.
3. **Submission rules** — No duplicate AUR packages, `x86_64`, naming (`-git`, `-bin`), `replaces` vs `conflicts`, maintainer comment header, usefulness, etc. **Mostly maintainer responsibility**; optional future: link to wiki or lightweight hints. **Official-repo shadowing** is **automated** for Register via **`pkgbase::check_pkgbase_publish_namespace`** (`official_repo_hit`); see [Pre-commit quality gate](#pre-commit-quality-gate-implemented-checks).

**Git commit identity**

- Wiki warns that commits use **global** `user.name` / `user.email` and are hard to rewrite. Consider a short UI note before first push (no new backend required).

---

## Recommended technical approach

### Align with wiki: clone-first (app standard)

`admin::register_on_aur` rustdoc describes **clone-first** (not `git init` + `git remote add` for Register). Layout matches Publish: `<work_dir>/aur/<pkgbase>` via `aur_git::aur_clone_dir`.

**Wiki nuance (two official paths)**

- **From scratch:** `git -c init.defaultBranch=master clone ssh://aur@aur.archlinux.org/<pkgbase>.git` — empty-repo warning is normal.
- **Already a local git tree:** `git -c init.defaultBranch=master init`, `git remote add …`, then **fetch** — equally valid on the wiki. The app may **only** implement the clone path for Register to avoid two UX flows; document that choice.

**Match the wiki’s clone invocation**

- **Done:** `ensure_clone` runs `git -c init.defaultBranch=master clone …` and, for **new** clones only, **`ensure_named_master_branch`** (`git branch -M master`). Existing clones are not auto-renamed.

**`master` branch only (wiki, settled)**

- The wiki states the AUR **only allows pushes to `master`**. `commit_and_push` uses `git push origin HEAD`. **MVP:** before stage/commit, **`register_require_master_branch`** errors if the current branch is not `master` (user must fix manually in edge cases). **Not done:** auto-repair for pre-existing off-master clones used with Register.

**Deleted pkgbase**

- If `<pkgbase>` matched a **deleted** package, the remote is often **not** empty; the wiki says to **fetch**, **pull/rebase**, and resolve conflicts. Do **not** run a naive “first import” that ignores remote commits. Product behavior for that case is spelled out in [Existing remote history](#existing-remote-history).

### Existing remote history

**Recommended UX.**

Treat the remote as **non-empty** when refs exist (e.g. a previously **deleted** AUR package whose **Git repo was retained**). The app should **not** pretend the remote is a blank slate.

**Ordering: `ls-remote` vs clone vs `git log` (for a good user decision)**

- **`git ls-remote <url>`** (optional **pre**-clone): cheap — lists ref names and **tip SHAs** only. Enough to know “history exists,” **not** enough to show **commit subjects**, authors, or depth. Different output from `git log`; **not** a substitute for it when the goal is an informed choice.
- **Clone** (or fetch that pulls commit objects): **required** before **`git log origin/master`** can show human-meaningful history, because log needs local objects / `refs/remotes/origin/master` (a normal `git clone` of a non-empty remote already establishes that).
- **Inspect, then present:** after clone, run a bounded **`git log origin/master --oneline`** (or equivalent) into **LogView** so the user can judge what they are inheriting.
- **Then** **warn** (if not already) and offer **Continue** vs **Abort** — so the flow is: **clone → check → show what matters → user decides** (with optional `ls-remote` earlier to message or branch UX before cloning if desired).

**What the wiki implies**

- **Continue** the repository: new work is integrated **on top of** `origin/master` via **fetch** (if refs need refreshing) and **rebase or merge**, then **push**. That is the documented model; it is **not** “replace history with a new root commit.”
- Maintainers **do not** have a normal, supported path to **delete** server-side history and “start clean” from the app; do not offer **force-push** or **history wipe** as a default or casual option.

**Recommended UI / flow**

1. **Optional** `git ls-remote` against the AUR URL: quick empty vs non-empty signal before heavier work.
2. **Clone** into `aur/<pkgbase>` when appropriate so **commit history** is available locally (same wiki-aligned clone as empty case; non-empty clone is expected for deleted pkgbases).
3. If remote has commits: **warn** clearly (e.g. history on host; align before push) and **show** bounded **`git log origin/master --oneline`** in **LogView** — not only tip SHAs from `ls-remote`.
4. **User choice**
   - **Continue on existing history** (primary path aligned with ArchWiki): **rebase or merge** the maintainer’s tree onto **`master`** (fetch first if local tracking refs could be stale), resolve conflicts if any, then **stage → commit → push** as usual. History is **extended**, not removed.
   - **Abort**: user picks another pkgbase or handles the repo outside the app if they do not want to adopt that history.

### Implementation status

| Area | Status |
| ---- | ------ |
| Clone-first, `-c init.defaultBranch=master`, `ensure_named_master_branch` on new clone | **Done** |
| Namespace early (`check_pkgbase_publish_namespace`, block official + AUR for greenfield) | **Done** |
| `validate::run_all` + required tier + `write_srcinfo` before stage | **Done** |
| Optional `ls_remote` (failure non-fatal) + post-clone `origin/master` commit count + bounded `git log` | **Done** |
| Strict vs continue: **`RegisterRemoteHistoryMode`** + wizard checkbox | **Done** (strict = abort after log; allow = `fetch_origin` + verify `origin` URL, then stage/push) |
| Full wiki **rebase/merge** when local clone diverged from `origin/master` | **Not done** — allow path assumes fresh clone at remote tip |
| `register_require_master_branch` | **Done** |
| First commit message | **`Initial import`** (fixed); template like Publish = open |
| Post-success “open package on Home” | **Not done** |
| Shared `run_all` + `write_srcinfo` with Publish Prepare | **Not done** (P1) |

**Polish (optional):** wizard / docs copy for the normal **empty-repo** `git clone` warning; **`admin.rs`** rustdoc for clone-first vs wiki **init + fetch** is in place.

### High-level steps (workflow)

| Step | Action | Notes |
| ---- | ------ | ----- |
| 1 | Preconditions | `PackageDef` comes from the **Register wizard**, not `state.package`. SSH verified (`ssh_ok` or equivalent), `git` / `makepkg` / tools per **`preflight`** (mirror Connection + Publish). `pkg.id` = intended **pkgbase** (`pkgbase::validate_aur_pkgbase_id` where applicable). **`pkgbase::check_pkgbase_publish_namespace`** — **abort** if `official_repo_hit` or `aur_pkgbase_hit` (greenfield Register only). |
| 2 | Resolve paths | `sync::package_dir` for PKGBUILD source; `aur_git::aur_clone_dir(work_dir, pkg.id)` for AUR clone — same as Publish. |
| 3 | **Validate (implemented checks)** | `validate::run_all(&build_dir, &tx)`; **abort** if required tier does not all `Pass`; stream all `LogLine`s. See [Pre-commit quality gate](#pre-commit-quality-gate-implemented-checks). |
| 4 | Regenerate `.SRCINFO` | `build_wf::write_srcinfo` in the **source** tree after validate (matches Publish). |
| 5 | Obtain clone | `git -c init.defaultBranch=master clone …` into `aur/<pkgbase>` when `.git` missing; if clone exists, define reuse vs refuse (same as Publish). Clone is what makes **`git log origin/master`** possible for an informed choice when the remote is non-empty. |
| 6 | Remote history decision | Optional **`git ls-remote <url>`** before clone for a cheap empty check. After clone, if **`origin/master`** has commits: **do not** naive first-import; **warn**, stream bounded **`git log origin/master --oneline`**, then user **continue** (rebase/merge per wiki) or **abort**. Non-empty remote means **history exists**, not necessarily that the package is still listed on the AUR web UI. Ensure `origin` URL matches `ssh://aur@aur.archlinux.org/<pkgbase>.git`. |
| 7 | Stage into clone | `aur_git::stage_files` today: **`PKGBUILD`** + **`.SRCINFO`** only — see [Wiki expectations not fully automated](#wiki-expectations-not-fully-automated) for patches/install files. |
| 8 | First commit + push | **MVP:** fixed message **`Initial import`**. Push must land on **`master`** (`register_require_master_branch` + `git push origin HEAD`). |
| 9 | Post-success UI | Toast suggests **Publish** / Home; **optional navigation** to select package — still open. |

### Safety / correctness gates

1. **Official + AUR index:** `official_repo_hit` or `aur_pkgbase_hit` from **`check_pkgbase_publish_namespace`** → **abort** Register (toast / error text aligned with `package_editor.rs` for official; AUR hit → direct toward **Publish** / adopt, not first-time create).
2. **Remote history present**: detect via optional **`git ls-remote <url>`** and/or **after clone** via **`git log origin/master`**. **Clone before** showing meaningful history to the user; then **warn**, **show bounded log**, user **continues** (rebase/merge, resolve conflicts) or **aborts** — see [Existing remote history](#existing-remote-history). Do not treat “refs exist” alone as proof the pkgbase is still an active AUR listing.
3. **Local clone dirty / wrong remote**: Refuse or reset only with explicit user intent; default is refuse with explanation.
4. **pkgbase mismatch**: Compare `PKGBUILD`’s `pkgname` / `pkgbase` with `pkg.id` where feasible; warn or hard-error per product decision.
5. **Root**: Do not run `makepkg` as root (`workflow::privilege::nix_is_root()`).
6. **No silent host trust**: Rely on existing SSH setup flow.
7. **Validate gate**: Required tier must pass before clone/stage/commit; optional tier non-blocking.

---

## UI / product work

| Item | Description |
| ---- | ----------- |
| Home tab | **Done** — button opens Register wizard; tooltip describes wizard (no `state.package`). |
| Entry point | **Done** — `register.rs`; `manage::start_register_new_aur_package` **removed**. |
| Logging | **Done** — `spawn_streaming` + `LogLine` → wizard **LogView**. |
| SSH | **Done** — banner + disabled push when `!ssh_ok` (Publish-style). |
| Confirmation | **Not done** — optional “public AUR package” confirm dialog. |
| Errors | **Partial** — typed `AdminError` (namespace, root, remote history, validation, …); toasts use `Display`. Optional: wiki links / tighter copy parity with `package_editor`. |

---

## Code structure (suggested)

1. **`workflow::aur_git`**: **Mostly done** — `ls_remote`, `log_origin_master_oneline`, `fetch_origin`, `ensure_clone` + master rename on new clone. **Gap:** full **rebase/merge** when local tree diverges (see [Implementation status](#implementation-status)).
2. **`workflow::admin::register_on_aur`**: **Done** — namespace → validate → `write_srcinfo` → `ls_remote` (best-effort) → `ensure_clone` → remote-history branch → `register_require_master_branch` → stage → `commit_and_push`; `Result<(), AdminError>`.
3. **Shared helper**: **Not done** — Publish Prepare still skips `run_all` (P1).

---

## Testing strategy

| Layer | What to test |
| ----- | -------------- |
| Unit | **`aur_git`:** `ls_remote` empty vs populated bare, `ensure_named_master_branch` rename — **done** (`target/aur_git_test_*`). Still useful: pkgbase consistency helpers, mocked `required_tier` summaries. |
| Integration | Temp dir + `git init --bare` fake origin — **no network** (optional P1; partial coverage via `aur_git` unit tests). |
| Manual | New pkgbase: validate → clone empty → stage → push; **official-repo name** → Register **aborts** before push; **AUR index hit** → abort with Publish/adopt hint; non-empty remote → warn + history + continue (rebase/merge) or abort; existing active package updates → Publish still works. |

Follow project rule: failing test first for bugfix regressions; for greenfield, add unit tests as helpers appear.

---

## Implementation todo / priorities

Use this list for sequencing and scope cuts. **P0** is required to meet [Goals](#goals) (wizard + correct backend + non-empty-remote safety). **P1** reduces drift and tightens UX. **P2** is follow-up polish.

### P0

- [x] **Backend correctness (MVP):** clone-first with `master` defaults, namespace gate, validate + `.SRCINFO`, stage/push; remote history **detect → log → strict abort or allow + fetch**; no silent overwrite. **Remaining:** full **rebase/merge** when local clone diverged ([Implementation status](#implementation-status)).
- [x] **Product entry:** Home → **Register wizard** (`ui/register.rs`); `PackageDef` from editor + `Registry::upsert`/`save`; `register_on_aur` — **not** `state.package`.
- [x] **Observability:** `spawn_streaming` + `LogLine`; SSH banner / push sensitivity when `!ssh_ok`.

### P1

- [ ] **Shared pre-stage gate:** factor `run_all` + `write_srcinfo` (optional `ls-remote` guard) for **Publish** Prepare so Register and Publish stay aligned. *Ties to:* [Publish vs this plan](#publish-vs-this-plan-important); [Code structure](#code-structure-suggested); checklist “shared helper” theme.
- [ ] **Errors & confirmation:** optional “public AUR package” confirm dialog; optional wiki links / `package_editor`-parity copy.
- [x] **Tests (partial):** `aur_git` unit tests for **ls-remote** + **ensure_named_master_branch**. **Remaining:** bounded-log helper test (optional), pkgbase consistency, fuller integration test.

### P2

- [ ] **Docs / lifecycle:** cross-link [GUI lifecycle](GUI_Lifecycle_Plan_Combined.md) **C3** when that row is updated; resolve [Open questions](#open-questions-resolve-during-implementation) in code or UI copy. *Ties to:* checklist 9.
- [ ] **Wiki-adjacent hints:** extra tracked files reminder, commit identity note, optional `makepkg --nobuild` if product wants script parity — **not** blocking MVP. *Ties to:* [Wiki expectations not fully automated](#wiki-expectations-not-fully-automated); [Pre-commit quality gate](#pre-commit-quality-gate-implemented-checks).

### Suggested execution order (within P0)

- [x] `ensure_clone` / default branch (`init.defaultBranch=master`) + `ensure_named_master_branch` on new clone.
- [x] Namespace early in `register_on_aur` (`check_pkgbase_publish_namespace`).
- [x] Validate + `write_srcinfo` + happy-path empty remote push.
- [x] Non-empty remote UX (strict vs allow + fetch + log). **Remaining:** full rebase/merge for diverged local clone.
- [x] Register wizard + Home wiring + streaming + SSH.
- [x] Checklist 8 (`cargo fmt` / `clippy` / `check` / `cargo test --bin aur-pkgbuilder`) — run before merge on future changes.

---

## Implementation checklist (ordered)

1. [x] Update `admin.rs` rustdoc: **clone-first** + wiki’s init/fetch alternative noted as manual-only.
2. [x] **`ensure_clone`**: `-c init.defaultBranch=master` + tests for related `aur_git` helpers.
3. [x] Wire **`pkgbase::check_pkgbase_publish_namespace`** at start of Register; **abort** on `official_repo_hit` or `aur_pkgbase_hit`; **`AdminError::PacmanNamespace`** for pacman probe failures.
4. [x] Non-empty remote: optional **`git ls-remote`** (non-fatal); **clone** + bounded **`git log origin/master`**; strict **abort** vs allow + **`git fetch origin`** + origin URL check. **Partial:** full **rebase/merge** for diverged working tree — not implemented.
5. [x] **`validate::run_all`** + required-tier handling + **`write_srcinfo`** before staging. **Not shared** with Publish yet.
6. [x] **`stage_files`** + **`commit_and_push`**; **`register_require_master_branch`** before stage; **`ensure_named_master_branch`** on **new** clone only.
7. [x] **Home** → **`ui/register.rs`** wizard; streaming + SSH gating; no `state.package` for Register.
8. [x] CI-style checks: `cargo fmt`, `clippy -D warnings`, `check`, `cargo test --bin aur-pkgbuilder`.
9. [ ] Optional: cross-link from `dev/docs/GUI_Lifecycle_Plan_Combined.md` **C3** when that row is updated.

---

## Open questions (resolve during implementation)

1. **Single vs two directories:** **Resolved for MVP** — Register uses **`sync::package_dir`** (same as Publish) for PKGBUILD / `.SRCINFO` source.
2. **Commit message:** **MVP = fixed `Initial import`**. Optional later: `config::render_commit_template` / `default_commit_message` like Publish.
3. **Extended staging:** Unchanged — still PKGBUILD + `.SRCINFO` only; align with a future Publish enhancement.

---

## Summary

**MVP shipped:** **Register new AUR package** is a **standalone wizard** (`ui/register.rs`): **Home** opens it; **`PackageDef`** comes from **package_editor** + **`Registry::upsert`/`save`**, not **`state.package`**. Backend runs **`pkgbase::check_pkgbase_publish_namespace`** (block official + AUR greenfield), **`workflow::privilege::nix_is_root`**, **`validate::run_all`** (required tier), **`write_srcinfo`**, optional **`ls-remote`**, **`ensure_clone`** (`init.defaultBranch=master` + **`ensure_named_master_branch`** on new clone), **`origin/master`** inspection (count + bounded log), **strict abort** vs **allow + `git fetch`** + origin URL check, **`register_require_master_branch`**, **`stage_files`**, **`commit_and_push`** (`Initial import`). **LogView** + **SSH banner** mirror Publish patterns.

**Still open vs full plan:** **Publish** does not yet share the validate+SRCINFO gate; **no** full **rebase/merge** when the local clone has diverged; **no** post-success “open package on Home”; **no** push confirm dialog; commit message not templated; extended file staging unchanged. Maintainer-driven items (extra tracked files, licenses, wiki policy) unchanged — see [Wiki expectations not fully automated](#wiki-expectations-not-fully-automated).
