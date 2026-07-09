# Potential Issues

Findings from a full-codebase audit (2026-07-09) focused on **reliability**,
**robustness**, and **user experience**. Line numbers refer to the tree at the
time of the audit and may drift. Each entry states the concrete failure
scenario and a suggested fix; issues are ordered by severity, most severe
first.

Categories: `reliability` (crashes, hangs, data loss), `robustness` (edge
cases, silent failures, invariant violations), `UX` (feedback, confirmation,
clarity).

---

## High severity

### 1. Corrupt `config.jsonc` / `packages.jsonc` silently resets to defaults, then the next save destroys the file

`src/workflow/registry.rs:58-72`, `src/config.rs:76-90` — reliability

`Registry::load` and `Config::load` swallow parse errors (`if let Ok(…)`) and
fall back to defaults. The JSONC headers explicitly invite hand-editing, so a
single trailing comma means the user launches into an empty registry / reset
settings with no explanation, onboarding may re-trigger, and the very next
save — including background saves like `record_pkgbuild_refresh`
(`src/workflow/package.rs:142`) — overwrites the still-recoverable file. That
is silent, permanent loss of the entire package catalog. `Config::load` can
also silently fall back to a stale legacy `config.json`, resurrecting old
settings.

**Fix:** distinguish "file missing" from "file unparsable"; on parse failure,
surface the error (with the file path) in the UI, keep a `.bak`/`.broken`
copy, and refuse to save over the unparsed file.

### 2. In-place, non-atomic file writes can truncate user data on crash / full disk

`src/workflow/registry.rs:86`, `src/config.rs:102`,
`src/workflow/sync.rs:244-247`, `src/workflow/ssh_setup.rs:655-658` —
reliability

`fs::write` truncates then writes in place. A crash, power loss, or `ENOSPC`
mid-write leaves a truncated `packages.jsonc`, `config.jsonc`, `PKGBUILD`
(download path), or — worst of all — the user's entire `~/.ssh/config`, a
file this app does not own. Combined with issue 1, a half-written registry is
then silently replaced by an empty one on next launch. The codebase already
has the correct pattern (`pkgbuild_edit::write_pkgbuild` and
`aur_git.rs:29-42` use temp-file + rename) — it just is not applied here.

**Fix:** write to a temp file in the same directory, set permissions, then
`rename()` over the target at all four sites.

### 3. Register/Publish validation gate blocks on optional-tier results — always fails without shellcheck/namcap

`src/workflow/admin.rs:320-330`, `src/workflow/validate.rs:193-195` —
reliability

`register_run_validate` runs `validate::run_all` (Required **and** Optional
tiers) and feeds the full list to `required_tier_all_pass`, which — despite
its name — requires *every* report to be `Pass` without filtering by tier.
On any machine without shellcheck or namcap those checks return `Skipped`, so
Register fails every time; even with them installed, a non-blocking shellcheck
`Warn` blocks the flow. `summarize_required_failures` then filters to
Required-tier failures, finds none, and shows the meaningless "required tier
incomplete (internal)". This violates the project's own graceful-degradation
invariant for optional tools.

**Fix:** make `required_tier_all_pass` filter to `CheckTier::Required` (or
filter the reports before calling it) so Optional `Warn`/`Skipped` never
block.

### 4. Panic in `strip_outer_quotes` on a lone quote character — reachable from downloaded upstream text

`src/workflow/pkgbuild_edit.rs:313-319` — reliability

For input `"` (a single quote character), `starts_with` and `ends_with`
match the same character and `s[1..s.len().saturating_sub(1)]` becomes
`s[1..0]`, which panics. A PKGBUILD containing a line like `pkgdesc="`
reaches this via `parse_quick_fields`, which is also run on *fetched
upstream* text in `admin::check_upstream` (`src/workflow/admin.rs:541-547`) —
so a malformed remote file crashes the app.

**Fix:** require `s.len() >= 2` before stripping the quotes.

### 5. `run_updpkgsums` restore logic silently discards updated arch-specific checksums

`src/workflow/build.rs:110-123,134,192-209` — reliability

`CHECKSUM_KEYS` covers only plain `sha256sums`/`sha512sums`/`md5sums`/
`b2sums`; arch-suffixed arrays (`sha256sums_x86_64=…`, standard for `-bin`
packages) and `sha1sums` are invisible to `checksum_arrays_equivalent`. If
updpkgsums rewrites only `sha256sums_x86_64` while the plain array is
unchanged, the comparison declares "equivalent" and the pre-run PKGBUILD is
**restored**, throwing away the correct new checksums while reporting
`pkgbuild_changed: false`. The next build fails integrity checks with stale
sums and no hint why. The same discard happens when a changed checksum array
is multi-line (extraction returns `None`) while another single-line key
matched.

**Fix:** only restore when the entire file differs solely in whitespace, or
extend extraction to arch-suffixed/multi-line arrays and treat any
unextractable checksum key as "changed".

### 6. Stage and Push can run concurrently on the same AUR clone directory

`src/ui/publish.rs:242-302`, `src/ui/publish.rs:313-356` — reliability

The Stage handler disables only `stage_btn` and Push disables only
`push_btn`. After a successful stage, the user can click Stage again and then
Push while the re-stage is still running — `ensure_clone`/`stage_files` race
`commit_and_push` in the same `aur_clone_dir` (git index lock failures at
best, committing a half-staged tree at worst). The reverse also holds.

**Fix:** disable both buttons whenever either operation is in flight and
re-enable both in the done callbacks.

### 7. `RefCell` borrow panic: `tab_pages` borrow held across refresh helpers

`src/ui/shell.rs:300-305` (with `:365-372`, `:344-346`, `:194`) — reliability

In `refresh_version_tab_page`, the `RefMut` from
`tab_pages.borrow_mut()` is still alive when `spawn_pkgver_tab_refresh` and
`spawn_validate_badge_refresh` run; both synchronously call helpers that do
`tab_pages.borrow()` whenever `sync::package_dir` returns `None`. Scenario: a
package without `destination_dir` is selected, the user clears the work-dir
entry (setting `config.work_dir = None`), and a pending Sync/Manage bulk
download completes and calls `refresh_version_tab_page` → `BorrowError`
panic, app aborts.

**Fix:** scope the `borrow_mut` block so the `RefMut` drops before invoking
the two `spawn_*` helpers.

---

## Medium severity

### Hangs and missing timeouts

### 8. No timeout on any PKGBUILD / AUR RPC HTTP request

`src/workflow/sync.rs:11-15`, `src/workflow/aur_account.rs:74-77,309-312` —
reliability

`pkgbuild_http_client` and both AUR RPC clients set no `.timeout()` /
`.connect_timeout()`, and reqwest's default has no total-request timeout. A
stalled server or half-open connection leaves the Sync probe, PKGBUILD
download, upstream check, username save, and pkgbase check spinning forever.
(The homepage fetch in `ssh_setup.rs:847` already does this correctly with a
15 s timeout.)

**Fix:** add `.timeout(Duration::from_secs(15-30))` and a connect timeout to
all client builders.

### 9. `ssh-add` can hang forever on a passphrase prompt

`src/workflow/ssh_setup.rs:462-495`, `src/ui/ssh_setup.rs:421-499` —
reliability

`ssh_add_private_key_env` nulls stdin, but ssh-add prompts for passphrases on
the controlling tty. If the app was launched from a terminal and the user
picks a passphrase-protected key, the prompt appears in the hidden terminal
while the UI shows "Adding…" with a disabled button indefinitely.

**Fix:** wrap the call in `tokio::time::timeout` and set
`SSH_ASKPASS_REQUIRE=never` (detached from the tty) so it fails fast with an
actionable message.

### 10. git-over-SSH in `aur_git` never sets `BatchMode=yes` and has no timeout

`src/workflow/aur_git.rs:80-92,212-227,592-597,601-631` — reliability

`preflight.rs` and `aur_ssh.rs` pass `-o BatchMode=yes -o ConnectTimeout=10`,
but every git command in `aur_git.rs` (clone, ls-remote, fetch, push) spawns
plain `git`. If `known_hosts` lacks the AUR entry or the key needs a
passphrase, ssh prompts on the tty and the publish/register flow hangs
forever with no output (`run_capture` buffers via `.output()`).

**Fix:** set `GIT_SSH_COMMAND="ssh -o BatchMode=yes -o ConnectTimeout=10"`
via `.env()` on the git commands; consider an overall timeout.

### 11. Embedded `ssh-agent` processes are never killed — they leak past app exit

`src/workflow/ssh_setup.rs:126-151`, `src/state.rs:27` — reliability

`spawn_ssh_agent_session` starts a detached `ssh-agent` and stores its
socket/PID, but nothing kills it on window close or shutdown. On desktops
where `SSH_AUTH_SOCK` isn't exported to GUI apps, every session that touches
"Check agent"/"ssh-add" leaves one more long-lived agent holding decrypted
private keys.

**Fix:** kill the agent (`ssh-agent -k` or signal by PID) on application
shutdown, or reuse a previously spawned agent by probing its socket.

### SSH / security-invariant gaps

### 12. `probe_aur_ssh` auto-trusts host keys with `StrictHostKeyChecking=accept-new`

`src/workflow/preflight.rs:357-365` — robustness

The connection-screen probe silently appends the AUR host key to
`known_hosts` with no fingerprint shown — directly contradicting the
project's "never auto-trust host keys silently; surface the SHA256
fingerprint" rule and bypassing the ssh_setup flow that exists to do it
correctly. A user on a hostile network gets a MITM key pinned unseen.

**Fix:** probe with `StrictHostKeyChecking=yes` and, on unknown-host failure,
route through the ssh_setup known-hosts flow that surfaces the fingerprint.

### 13. `upsert_host_block` parses `~/.ssh/config` case-sensitively and only exact-matches host lines

`src/workflow/ssh_setup.rs:927-980` — robustness

OpenSSH keywords are case-insensitive. Two concrete failures: (1) if the
block after `Host aur.archlinux.org` starts with lowercase `host github.com`,
the skip loop never treats it as a boundary and deletes the user's GitHub
block; (2) if the existing entry is `host aur.archlinux.org` or
`Host aur.archlinux.org aur-alias`, the match fails, a duplicate block is
appended at EOF, ssh's first-match-wins keeps the old `IdentityFile`, and the
UI still toasts "SSH config updated".

**Fix:** match `Host`/`Match` keywords case-insensitively and treat any host
line whose pattern list contains `aur.archlinux.org` as the target.

### 14. "Update known hosts" cannot recover from a rotated/mismatched AUR host key

`src/workflow/ssh_setup.rs:591-593,713-724` — UX

`host_entry_exists` (`ssh-keygen -F`) only checks that *some* entry exists,
not that it matches current published fingerprints. After the AUR rotates its
host keys, ssh fails with "REMOTE HOST IDENTIFICATION HAS CHANGED", and the
one button designed to fix it reports "already trusted" while doing nothing —
no in-app path to remove the stale line.

**Fix:** when the entry exists, fingerprint it against the trusted list and
offer `ssh-keygen -R` + re-scan on mismatch.

### Workflow correctness

### 15. `commit_and_push` gates on whole-tree `git status --porcelain` — untracked files cause a hard failure

`src/workflow/aur_git.rs:575-597` — reliability

After `git add PKGBUILD .SRCINFO`, the emptiness check includes untracked
(`??`) lines — e.g. a stray `*.pkg.tar.zst` in the clone. With
PKGBUILD/.SRCINFO unchanged but junk present, the guard passes, `git commit`
exits 1 with "nothing to commit", and the publish flow aborts with the opaque
"command failed: exit status: 1".

**Fix:** gate on `git diff --cached --quiet` instead.

### 16. Imported split packages get a broken `pkgbuild_url` — `PackageBase` is parsed then discarded

`src/workflow/aur_account.rs:197-218,337-348` — robustness

The cgit plain view is keyed by **pkgbase** (`?h=<pkgbase>`), but
`to_package_def` builds the URL from `summary.name` (pkgname). `RpcResult`
deserializes `PackageBase`, yet `into_summary` drops it — for a split package
(name ≠ base) the registry entry's PKGBUILD fetch 404s after import from
"My AUR packages".

**Fix:** carry `package_base` into `AurPackageSummary` and use it in
`aur_pkgbuild_url`.

### 17. `download_pkgbuild` accepts any HTTP-200 body — an HTML page can overwrite the real PKGBUILD

`src/workflow/sync.rs:244-247` — robustness

Any 200 body is written over the local `PKGBUILD`: a GitHub *blob* URL
(instead of raw) or a captive-portal page destroys local edits before any
check runs. (The non-atomic write itself is covered in issue 2.)

**Fix:** reject bodies that plainly aren't a PKGBUILD (leading
`<!DOCTYPE`/`<html`, or non-text content-type) before writing.

### 18. Tool detection depends on the external `which` binary, which the project itself lists as possibly missing

`src/workflow/preflight.rs:324-342`, `src/workflow/validate.rs:478-488` —
robustness

Both `which()` and `is_available()` shell out to `which`. On a system without
it, the Connection screen reports all four required tools missing with
misleading `pacman -S` hints, and every optional check reports "not
installed" even when present — so shellcheck/namcap never run and (per
issue 3) Register fails.

**Fix:** resolve programs by scanning `PATH` with `std::env::split_paths`
instead of shelling out to `which`.

### UI state and async lifecycle

### 19. A panic in any spawned workflow task silently strands the UI (spinner forever, buttons dead)

`src/runtime.rs:34-41,58-70`; example victim `src/ui/build.rs:101-102` —
reliability

Tokio catches panics in spawned tasks; the channel sender is dropped,
`rx.recv()` errors, and `if let Ok(…)` skips `on_done` entirely. Every caller
disables its button and starts its spinner *before* spawning, so a single
panic in workflow code leaves the page permanently stuck with no error shown.

**Fix:** in `spawn`/`spawn_streaming`, treat channel-closed-without-value as
a completion event (deliver a fallback error to `on_done`) so UI cleanup
always runs.

### 20. updpkgsums rewrites the PKGBUILD on disk but the editor on the same page keeps stale text — Save silently reverts the checksums

`src/ui/version.rs:42-49,146-166` — reliability

The Version page hosts both the PKGBUILD editor and the "refresh sha256sums"
runner. On `pkgbuild_changed` success the handler only toasts; it never
reloads the editor buffer (Sync's download does, via
`refresh_version_tab_page`). A user who runs updpkgsums, edits once more in
the still-open editor, and hits Save writes the pre-updpkgsums buffer back —
then builds/publishes with wrong sums.

**Fix:** reload the editor content (or refresh the Version tab page) on
`pkgbuild_changed`.

### 21. Periodic SSH probe rebuilds the Publish tab, destroying in-progress publish state

`src/ui/shell.rs:266-273,309-334,421-429`, `src/ui/ssh_probe.rs:97-99` — UX

`start_periodic_connection_checks` re-probes every 300 s; whenever `ssh_ok`
flips (one transient network hiccup), `refresh_publish_tab_page` replaces the
whole Publish page. Typed commit message, staged diff, push-enabled state,
and the streaming log widget are discarded — if a push is streaming, its
output vanishes while the subprocess keeps running.

**Fix:** update the existing page's sensitivity/banner in place, or at least
skip the rebuild while a publish operation is in flight.

### 22. "Run All" validation is never disabled — concurrent runs in the same package dir

`src/ui/validate.rs:242-274,323` — reliability

Unlike the optional/extended buttons, `run_all_btn` stays sensitive while a
run is in flight. Double-clicking (or clicking during the automatic Required
run at page build) launches two concurrent `makepkg --verifysource` processes
downloading into the same directory, `log.clear()` wipes the first
transcript mid-stream, and the runs' report icons overwrite each other out of
order.

**Fix:** disable all tier buttons for the duration of any in-flight run,
mirroring the extended-tier pattern.

### 23. Merely selecting a package auto-runs `makepkg --verifysource` (network downloads)

`src/ui/validate.rs:323-331`, triggered by `src/ui/shell.rs:593-615` — UX

The Validate page constructor immediately spawns the Required tier, which
includes `VerifySource` — unrequested, potentially large source downloads.
For a fresh package with no PKGBUILD yet, `package_dir` still resolves, so
the user gets a "required checks failed" toast seconds after picking a
package, before ever visiting Sync or Validate.

**Fix:** defer the auto-run until the Validate tab is first shown, and skip
it (or run only `bash -n`) when the PKGBUILD doesn't exist yet.

### 24. Cancelling the package editor during the pkgbase namespace check does not cancel the save

`src/ui/package_editor.rs:137-146,551,640-676` — robustness

Save starts an async `check_pkgbase_publish_namespace` probe, but Cancel (and
window close) never disarm the `once` callback. If the user cancels while the
probe is slow and the name turns out clean, the package is still
added/registered and a "saved" toast appears despite the explicit cancel.

**Fix:** take/clear the callback (or set a cancelled flag checked in the
probe callback) in the Cancel handler and `connect_close_request`.

### 25. Removing the currently selected package leaves `state.package` / `config.last_package` dangling

`src/ui/home.rs:984-993` (compare `:1087-1107`) — robustness

The per-row remove handler only calls `registry.remove(&id)`; unlike
`perform_remove_mismatched_packages`, it never clears `st.package` or
`st.config.last_package`. The Sync–Publish tabs stay built for the deleted
package, and the deleted id is persisted as `last_package`, so the next
launch restores a package that no longer exists.

**Fix:** mirror the mismatch-removal logic — clear both fields when they
match the removed id, then `refresh_tabs_for_package`.

### Destructive actions and confirmations

### 26. Home per-row trash button deletes a package with no confirmation

`src/ui/home.rs:984-993` — UX

One accidental click on the flat `user-trash-symbolic` icon next to the edit
button removes the entry and saves — no dialog, no undo. The bulk
"remove mismatched" path correctly uses an `AlertDialog`
(`src/ui/home.rs:1117-1160`).

**Fix:** show a destructive-styled `AlertDialog`, or a toast with an Undo
action.

### 27. Destructive AUR SSH commands (adopt / disown / setup-repo) execute on a single click

`src/ui/aur_ssh.rs:135-156,163` — UX

The rows wear the `destructive` badge, but `connect_clicked` goes straight to
`aur_ssh::run`. One misclick on "Disown" immediately transfers away ownership
of the pre-filled package, with no undo.

**Fix:** show an `adw::AlertDialog` naming the command and package before
running destructive commands.

### 28. PKGBUILD editor discards unsaved edits without warning (Reload and tab rebuilds)

`src/ui/pkgbuild_editor.rs:598-652`; rebuild paths `src/ui/sync.rs:331`,
`src/ui/shell.rs:278-306` — UX

Reload replaces the buffer from disk with no unsaved-changes prompt even
though the editor tracks a baseline. Worse, the Version tab is rebuilt
wholesale after a Sync/Manage bulk download, so edits in the Version-tab
editor are silently destroyed by an action on a different tab.

**Fix:** compare buffer text against the baseline and confirm before Reload
or a rebuild replaces a dirty editor.

### Feedback and staleness

### 29. Manage tab package list goes stale after registry changes

`src/ui/manage.rs:318-344`; built once at `src/ui/shell.rs:138` — UX

`packages_group` snapshots the registry at build time and is rebuilt only on
locale change. Adding/importing/removing packages (even importing on Manage
itself, line 218) leaves the admin list showing the old set — new packages
have no row, deleted ones still show actions that will fail.

**Fix:** add a `refresh_manage_tab_page` on `MainShell` and call it from
registry-mutation paths, or rebuild the group on tab selection.

### 30. "Check all packages" has no busy state, no progress, and allows concurrent runs

`src/ui/manage.rs:252-310` — UX

The bulk check runs one sequential network fetch per package and can take
many seconds, but the button is never disabled and nothing appears until the
loop finishes — so the user clicks again and eventually gets two report
windows and duplicate toasts.

**Fix:** disable the button and show a spinner (or stream per-package
progress into a `LogView`), re-enabling in the done callback.

### 31. Sync URL probe runs once with no retry — transient failure disables Download for the session

`src/ui/sync.rs:172-220` — UX

The reachability probe runs only at page construction, and tab pages rebuild
only when the selected package changes. A transient outage sticks
`source_reachable` at `Some(false)` and the Download button stays
insensitive until the user switches packages or restarts.

**Fix:** add a retry affordance (re-probe button, or re-probe when the tab
becomes visible after a failure).

### Persistence gaps

### 32. `save()` errors silently discarded at many sites — success toasts on failed persistence

`src/ui/onboarding.rs:234`, `src/ui/ssh_setup.rs:161,279,570,583`,
`src/ui/ssh_probe.rs:26-29`, `src/workflow/package.rs:142,162` — robustness

Numerous `let _ = ….save();` calls swallow errors. If the config dir is
read-only or the disk full: onboarding still toasts "Imported N packages"
(all lost on next launch, onboarding reappears), "Use this key" toasts
success but the selection doesn't persist, and PKGBUILD-refresh timestamps
never stick so staleness warnings keep firing — all with zero indication.
The username dialog path (`ssh_setup.rs:652`) and Sync page
(`sync.rs:58-64`) already show the correct pattern.

**Fix:** surface `save()` failures (toast with the file path) at every site,
matching the existing correct call sites.

### 33. `CONFIG_HEADER` promises user notes "will stay intact", but `save()` destroys them

`src/config.rs:14-19` vs `src/config.rs:92-104` — UX

The header written into `config.jsonc` says notes above/below the block stay
intact, but `Config::save` writes only `CONFIG_HEADER + serialized body`, so
any user note is discarded on the next settings change. Same pattern in
`Registry::save`. This is documented data loss.

**Fix:** preserve non-header leading/trailing lines when rewriting, or
correct the header text.

### 34. LogView leaks one anonymous TextMark per appended line and grows without bound

`src/ui/log_view.rs:179-184` — reliability

`append()` calls `buffer.create_mark(None, …)` per line and never deletes the
mark; anonymous marks survive `clear()`. A long `makepkg` build accumulates
thousands of live marks (plus unbounded buffer text) for the window's
lifetime, degrading memory and TextBuffer performance across runs.

**Fix:** create one persistent end-mark in `new()` and reuse it for
`scroll_to_mark`; consider capping buffer length.

---

## Low severity

### 35. `UpdateStatus::Outdated` claims "outdated" for any byte difference, including when local is newer

`src/workflow/admin.rs:537-556` — UX

No version comparison is performed — a local pkgrel bump or an added comment
is labeled `Outdated`, and Manage's "check all" nudges the user to
re-download and overwrite their newer local work.
**Fix:** compare parsed `epoch:pkgver-pkgrel` with vercmp semantics and add a
distinct "locally modified / ahead" state.

### 36. Subprocess spawn failures don't name the missing program or give an install hint

`src/workflow/build.rs:264-270` — UX

A missing `updpkgsums` surfaces as "spawning child process: No such file or
directory" — the user can't tell which tool is missing or what to install.
**Fix:** include the program name in the error context and, for known tools,
the exact `pacman -S --needed …` hint.

### 37. `open_work_dir` spawns `xdg-open` from a Tokio worker, contradicting the codebase's own guidance

`src/workflow/admin.rs:585-598` vs `src/workflow/preflight.rs:316-319` — UX

`preflight`'s rustdoc explicitly warns not to spawn `xdg-open` from a Tokio
worker (fails under Wayland/portals), yet `open_work_dir` does exactly that,
yielding an opaque "xdg-open exited 4" toast.
**Fix:** route through `GtkFileLauncher` on the main thread like the
Connection screen.

### 38. `remote_tree_has_pkgbuild` clones into the system temp dir, contrary to the file-system-safety rule

`src/workflow/aur_git.rs:197-245` — robustness

The probe clones into `std::env::temp_dir()` instead of the configured work
dir; a cleanup error (line 240) also turns a *successful* probe into a
failure, and the clone is orphaned if the process dies mid-probe.
**Fix:** clone under `<work_dir>/aur/.probe-…` and log (not fail) on cleanup
errors.

### 39. `ensure_aur_key` silently degrades when `~/.ssh/aur` exists but `aur.pub` is missing

`src/workflow/ssh_setup.rs:531-544` — robustness

`read_public_key(…).unwrap_or_default()` yields blank metadata and no
fingerprint; setup reports "Reused" as success, but "Copy public key" later
fails and the key row shows empty fields.
**Fix:** regenerate the `.pub` via `ssh-keygen -y -f ~/.ssh/aur`, or return
an actionable error naming the missing path.

### 40. One-click setup surfaces only the first of the newly trusted host-key fingerprints

`src/ui/ssh_setup.rs:175-187` — UX

`ensure_known_hosts_entry` typically adds three keys and returns all
fingerprints, but the one-click path shows only `fingerprints.first()` —
weakening the fingerprint-verification invariant. The standalone connectivity
button (`ssh_setup.rs:1096-1098`) already shows each one.
**Fix:** toast each fingerprint (or join them).

### 41. One-click SSH setup gives no progress feedback for a multi-network-step operation

`src/ui/ssh_setup.rs:120-141` — UX

`full_setup` chains keygen, an HTTPS fetch, `ssh-keyscan`, and per-key
fingerprinting (worst case 20 s+), but the only feedback is a disabled
button — on a slow network the flagship button just looks dead.
**Fix:** add the same pulsing stamp label / spinner used by the per-step
rows.

### 42. Concurrent AUR SSH commands clear and interleave the shared log view

`src/ui/aur_ssh.rs:163-166` — UX

Only the clicked row's button is disabled; starting a second command calls
`log.clear()` (wiping in-flight output) and interleaves both commands' lines
with no attribution.
**Fix:** disable all Run buttons (or queue) while a command is in flight.

### 43. SSH probe status strings are hardcoded English, bypassing i18n

`src/ui/ssh_probe.rs:52,74-94` — UX

"probing…", "connected", "key rejected", "failed (exit …)", and "error" are
literal strings while everything around them goes through `i18n::t` — mixed-
language status text on both screens sharing this helper.
**Fix:** route these strings through `i18n::t`/`i18n::tf`.

### 44. `--nobuild` run reports "Build succeeded" and unlocks "Continue to publish"

`src/ui/build.rs:132-136` — UX

`makepkg --nobuild` exits 0 after only fetching/extracting sources, but the
UI shows the generic success status and enables the publish continuation,
implying a package was built.
**Fix:** show a "sources prepared (no build)" status when `--nobuild` was
requested.

### 45. Hand-written partial config gets `None` work_dir instead of the documented default

`src/config.rs:32-33` vs `src/config.rs:52-62` — robustness

Field-level `#[serde(default)]` on `work_dir` defaults to `None`, not
`default_work_dir()`. A user who hand-creates `config.jsonc` with only
`{"aur_username": "x"}` gets no work dir and every action fails with
"no workdir" toasts.
**Fix:** use `#[serde(default = "default_work_dir")]` (similarly `ssh_key`).

### 46. LogView hardcoded dark-theme colors are hard to read in light theme

`src/ui/log_view.rs:54-63` — UX

Info (`#8ab4f8`) and stderr (`#f28b82`) foregrounds are dark-theme pastels
with poor contrast on a light Adwaita background.
**Fix:** derive tag colors from the theme (CSS classes / accent and error
colors) instead of fixed hex values.

### 47. Onboarding persists the username to config before it is validated

`src/ui/onboarding.rs:144-145` — UX

The typed username is saved before the RPC call, so a typo'd username that
errors is still persisted (suppressing future onboarding and feeding the
account-mismatch checks); the save error itself is also ignored.
**Fix:** persist only after a successful fetch, and surface `save()` errors.

### 48. Connection path edits are only persisted on Continue/browse/probe

`src/ui/connection.rs:238-245,256-263` — UX

`connect_changed` updates `work_dir`/`ssh_key` in memory on every keystroke
but never saves; typing a work dir and closing the app silently loses the
setting, while unrelated actions persist whatever half-typed path happens to
be in state.
**Fix:** debounce-save from the changed handlers or save on focus-out.

### 49. Register wizard "Create starter PKGBUILD" is not disabled while the async creation runs

`src/ui/register.rs:479-560` — UX

A double-click launches two concurrent creations (TOCTOU on the "if missing"
check against the same file) and can open two stacked modal editor windows.
**Fix:** disable the button at click and re-enable in the done callback.

### 50. Home favorite popovers are never unparented and are recreated on every list refresh

`src/ui/home.rs:442,809` — robustness

GTK4 requires `Popover::set_parent` widgets to be manually `unparent()`ed
before the parent is disposed; `list.remove_all()` disposes rows with
popovers attached, producing "Finalizing … still has children" criticals on
every search keystroke, plus per-refresh closure churn.
**Fix:** unparent in a destroy/unrealize handler, or share one popover for
the whole list.

### 51. PKGBUILD Save records the wrong baseline if the user types during the write

`src/ui/pkgbuild_editor.rs:669-680` — robustness

Save writes the click-time snapshot but sets the baseline to the buffer's
*completion-time* content, so keystrokes made during the async write are
treated as "saved" — the diff highlighting gives no signal that a second Save
is needed.
**Fix:** set the baseline to the `text` snapshot actually written.

---

## Additional findings from verification pass

### 52. Existing AUR clone directories are reused without verifying `origin` or branch

`src/workflow/aur_git.rs:51-67`, `src/ui/publish.rs:268-275,333-334`,
`src/workflow/admin.rs:140-145,183-188,409` — reliability

`ensure_clone` returns immediately when `<work_dir>/aur/<pkgid>/.git` exists;
it does not check that `origin` still equals the expected
`ssh://aur@aur.archlinux.org/<pkgid>.git`, nor that the current branch is
`master`. Publish then stages files into that checkout and `commit_and_push`
pushes `origin HEAD`. A stale/manual clone, wrong remote, or non-master branch
can therefore send the maintainer's PKGBUILD to the wrong repository or fail
only after local files have been copied into the wrong working tree. Register
has `register_verify_origin_remote`, but it is only reached in the
"allow existing remote history" branch, not for every reused clone.

**Fix:** whenever reusing an existing clone, fail closed unless `origin` and
current branch match the selected package. Prefer pushing explicitly to
`HEAD:master` after the checks pass.

### 53. AUR staging copies only three filenames, so local patches/install files are silently omitted

`src/workflow/aur_git.rs:496-503`, callers in `src/ui/publish.rs:271` and
`src/workflow/admin.rs:145,185` — reliability

`stage_files` copies only `PKGBUILD`, `.SRCINFO`, and `.gitignore` from the
build directory into the AUR clone. Many PKGBUILDs need local files tracked in
the AUR repo (`*.patch`, `.install`, systemd units, desktop files, helper
scripts, icons, etc.). Those files can be present locally and referenced from
`source=()` / `install=`, but they are never copied or staged, so the push can
succeed while the AUR package is broken for everyone else. The default
`.gitignore` comment says users may track helper scripts, but the copy
whitelist makes that impossible.

**Fix:** stage a complete, deliberate AUR source tree: parse local `source=()`
and `install=` references, or copy all unignored maintainer files from the
package directory and run `git add -A` with a safe exclude policy for build
artifacts.

### 54. The generated `.gitignore` ignores itself, then `git add .gitignore` fails

`src/workflow/aur_git.rs:17,29-40,568-573`,
`src/workflow/admin.rs:135-145,171-188` — reliability

`DEFAULT_AUR_GITIGNORE` is `*\n!.SRCINFO\n!PKGBUILD\n`, which leaves
`.gitignore` ignored by its own first rule. Register creates that file in the
package directory, copies it into the clone, and `commit_and_push` tries to
`git add .gitignore` whenever it exists. Git rejects that without `-f`
("pathspec is ignored by one of your .gitignore files"), so the initial import
or next publish fails after an otherwise successful prepare/stage.

**Fix:** include `!.gitignore` in the default file (or use `git add -f
.gitignore` intentionally) and add a regression test that `git add PKGBUILD
.SRCINFO .gitignore` succeeds in a fresh repo.

### 55. Publish Stage cannot preview an empty/unborn AUR clone

`src/workflow/aur_git.rs:51-91,520-558`, `src/ui/publish.rs:268-277` —
reliability

A brand-new empty AUR Git repository clones with no `HEAD` commit. Publish
Stage copies files, then calls `diff()` (`git diff --stat HEAD` / `git diff
HEAD`) and `has_changes_vs_head()` (`git diff --quiet HEAD`). In an unborn
repo those commands fail with exit 128 because `HEAD` is ambiguous; `diff()`
quietly discards the failure and reports "no changes", while
`has_changes_vs_head()` returns an error that disables Push. This blocks the
Publish path for empty remotes even though `commit_and_push` could create the
first commit.

**Fix:** detect unborn repositories with `git rev-parse --verify HEAD`. For
preview, use `git status --porcelain` or compare against Git's empty tree after
intent-to-add; for gating, treat required untracked `PKGBUILD` / `.SRCINFO` as
publishable changes.

### 56. Official-repository namespace check treats all `pacman -Si` failures as "name is free"

`src/workflow/pkgbase.rs:65-75,99-105` — robustness

`official_repo_pkg_exists` discards stdout/stderr and returns
`Ok(output.status.success())`. That means every non-zero `pacman -Si <name>`
result — missing sync databases, a corrupted pacman database, lock/config
errors, or an interrupted pacman setup — is interpreted exactly like "target
not found". `check_pkgbase_publish_namespace` can then report
`official_repo_hit: false`, and Register may proceed with an AUR pkgbase that
actually collides with an official repository package.

**Fix:** capture stderr and distinguish the known "target not found" case from
pacman operational failures. Fail closed with `PkgbaseNsError::Pacman` for any
ambiguous status.

---

## Suggested priorities

1. **Data-loss cluster (issues 1, 2, 32, 33):** atomic writes + honest error
   surfacing for `config.jsonc` / `packages.jsonc` / `~/.ssh/config` — the
   highest-value, lowest-risk fixes in the list.
2. **Register gate (issue 3) and updpkgsums restore (issue 5):** both make
   core flows fail or corrupt state on ordinary machines.
3. **Panic fixes (issues 4, 7, 19):** small, testable, and each currently
   aborts or strands the whole app.
4. **Hang cluster (issues 8–11):** timeouts + `BatchMode` are mechanical
   changes that remove every indefinite-hang path.
5. UX confirmation/feedback items can be batched per page (Home, Manage,
   Publish, Validate) as follow-ups.
