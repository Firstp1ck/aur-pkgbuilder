# Rust Development Rules for AI Agents

Rules for agents contributing to **aur-pkgbuilder** (GTK4 + libadwaita
desktop app that drives an AUR maintainer's release flow).

## When Creating New Code (Files, Functions, Methods, Enums)

- Keep **cognitive complexity** below **25**. `clippy::cognitive_complexity`
  is enabled at **warn** (see `Cargo.toml` / `clippy.toml`); treat `-D warnings`
  runs as enforcing the bar — match the shape of existing modules such as
  `workflow::ssh_setup` or `workflow::validate`.
- Keep functions under **150 lines**. Split at natural seams — `fn
  build(nav, state)` in `src/ui/*.rs` should delegate to small
  `fn *_group(…) -> PreferencesGroup` helpers, not inline everything.
- Prefer straightforward **data flow** (few threaded parameters, clear
  ownership boundaries). GTK widgets live on the main thread; long work
  is routed through `runtime::spawn` / `runtime::spawn_streaming`.
- Add `///` rustdoc to all new public items. Private items benefit from
  docs too — add them when the behavior is non-obvious.
- Use the **What / Inputs / Output / Details** rustdoc layout for
  non-trivial APIs (template below).
- Add focused **unit** tests for pure logic. Keep tests deterministic —
  no network, no files outside the test harness.

## When Fixing Bugs/Issues

1. Identify the root cause before writing code.
2. Write or adjust a test that **fails** on the bug.
3. Run the test — it must fail. If it passes, the test does not
   reproduce the issue; adjust it.
4. Fix the bug.
5. Run the test again — it must pass. If not, iterate on the fix.
6. Add edge-case tests when they reduce future regressions.

## Always Run After Changes

Run from the repository root, in this order:

1. `cargo fmt --all` (uses `rustfmt.toml`)
2. `cargo clippy --all-targets --all-features -- -D warnings` (uses `clippy.toml`)
3. `cargo check`
4. `cargo test --bin aur-pkgbuilder`
5. `cargo deny check` (uses `deny.toml`; optional locally if `cargo-deny` is not installed)

`cargo test --bin aur-pkgbuilder` is required because the crate has no
`lib` target — the plain `cargo test` command will error with "no
library targets found".

## Lint configuration (source of truth)

- **Clippy:** `clippy.toml` sets `cognitive-complexity-threshold` and
  `too-many-lines-threshold`. `[lints.clippy]` in `Cargo.toml` enables
  `cognitive_complexity = "warn"`. CI/agents run
  `cargo clippy --all-targets --all-features -- -D warnings`.
- **rustfmt:** `rustfmt.toml` at the repo root.
- **Dependencies / licenses:** `deny.toml` for `cargo deny check`.
- **Secrets:** `.gitleaks.toml` for `gitleaks detect`.
- **Shortcuts:** root `Makefile` delegates to `dev/Makefile` (`make fmt`,
  `make clippy`, `make test`, `make pre-commit`, …).

When changing any of the above, update this section and keep `CLAUDE.md`
in sync.

## Code Quality Requirements

### Pre-commit checklist

Before completing any task, ensure all of the following pass:

1. **Format:** `cargo fmt --all` produces no diff.
2. **Clippy:** `cargo clippy --all-targets --all-features -- -D warnings` is clean.
3. **Compile:** `cargo check` succeeds.
4. **Tests:** `cargo test --bin aur-pkgbuilder` — all tests pass.
5. **Complexity:** new functions stay under ~25 cognitive complexity.
6. **Length:** new functions stay under ~150 lines.
7. **Exceptions:** if a threshold cannot reasonably be met, add a
   **documented** `#[allow(...)]` with a justification comment. Use
   sparingly.
8. **cargo-deny:** `cargo deny check` passes when using `make pre-commit`
   (install `cargo-deny` if needed).

### Documentation

- For non-trivial APIs, use the structured rustdoc layout with **What**,
  **Inputs**, **Output**, and **Details** sections:

  ```rust
  /// What: Brief description of what the function does.
  ///
  /// Inputs:
  /// - `param1`: Description of parameter 1
  /// - `param2`: Description of parameter 2
  ///
  /// Output:
  /// - Description of return value or side effects
  ///
  /// Details:
  /// - Additional context, edge cases, or important notes.
  pub fn example_function(param1: Type1, param2: Type2) -> Result<Type3> {
      // implementation
  }
  ```

- Do not write comments that narrate what the code does (`// import the
  module`, `// increment the counter`). Comments should explain
  non-obvious intent, trade-offs, or constraints.

### Testing

**For bug fixes:**

1. Create a failing test that reproduces the issue.
2. Fix the bug.
3. Verify the test passes.
4. Add additional edge-case tests if applicable.

**For new features:**

1. Add unit tests for the core logic.
2. Prefer pure functions that are test-friendly. See
   `workflow::ssh_setup::upsert_host_block` and its three unit tests as
   a reference — separating parsing/formatting from I/O makes the
   asserts trivial.
3. Test error cases and edge conditions.

**Test guidelines:**

- Tests must be deterministic — no network, no dependence on the
  developer's home directory, no assumptions about other test order.
- Use the default test threading. There is no parallelism hazard today.

## Code style conventions

- **Edition:** Rust 2024 (see `Cargo.toml`).
- **Naming:** Clear and descriptive; clarity over brevity.
- **Errors:** Use `Result`. Typed errors in `workflow::*`
  (`admin::AdminError`, `ssh_setup::SshSetupError`,
  `aur_account::AurAccountError`). UI code matches on the specific
  error variant to render appropriate toasts.
- **Never** use `unwrap()` / `expect()` in non-test code. The
  exceptions already in the tree (`child.stdout.take().expect("stdout
  piped")` immediately after setting `Stdio::piped()`) are local to the
  call and documented by the piping call above them — match that
  pattern if you absolutely need it.
- **Control flow:** Prefer early returns over deep nesting. Reach for
  `let … else { … return; }` when a guard is sharper than an `if let`.
- **Logging:** there is no tracing infrastructure yet; user-facing
  diagnostics go through toasts and the log view. Don't add println!s
  to production code paths — route them through the `LogLine` stream
  instead so they appear where the user is already looking.

## Async boundary

- GTK/libadwaita widgets must live on the main thread.
- For one-shot Tokio work + single callback, use `runtime::spawn`.
- For streaming subprocess output to the UI, use
  `runtime::spawn_streaming` with an `async_channel::Sender<LogLine>`.
- Shared state (`state::AppStateRef` = `Rc<RefCell<AppState>>`) is
  single-threaded. Never hand it to a Tokio task — clone the fields you
  need instead.

## Platform behavior

### External tools

- `makepkg`, `git`, `ssh`, `ssh-keygen`, `ssh-keyscan`, `updpkgsums`,
  `xdg-open`, `fakeroot`, `shellcheck`, `namcap`, `which` — **any of
  these may be missing**.
- Required tools are surfaced on the connection screen
  (`src/workflow/preflight.rs` — `makepkg`, `git`, `ssh`, `updpkgsums`).
- Optional tools (`shellcheck`, `namcap`, `fakeroot`) are probed at use
  time and reported as *skipped* with an install hint — see the
  `is_available(program)` helper in `src/workflow/validate.rs`. Never
  crash because an optional tool is missing.

### Graceful degradation

- `makepkg` must not run as root. The `nix_is_root()` guard in
  `src/ui/build.rs` stays in place.
- `xdg-open` failing (no display, no handler) returns an error —
  handle the `Err` and toast it.

### Error messages

- User-facing errors must say **what** failed and **what the user can
  do** next. Missing tool? Quote the exact `pacman -S --needed …`
  command. Bad config? Point at the path of the file.

## Configuration updates

If config keys or schema change:

- Update `CONFIG_HEADER` in `src/config.rs` (for `config.jsonc`) or
  `REGISTRY_HEADER` in `src/workflow/registry.rs` (for `packages.jsonc`)
  so the fixed schema comment in saved files stays accurate.
- Add new fields with `#[serde(default)]` so existing JSONC files
  upgrade in place without erroring.
- The legacy `.json` fallback branches in `Config::load` and
  `Registry::load` remain in place — do not remove them.

## UX guidelines

- Follow libadwaita patterns: `PreferencesGroup` for grouped rows,
  `ActionRow` + suffix buttons for per-row actions, `Toast` for async
  results, `AdwNavigationView` for multi-step flows.
- Destructive actions wear the `destructive` pill label and, for
  primary buttons, the `destructive-action` CSS class.
- `preview` badge marks intentional stubs that return
  `NotImplemented(&'static str)` from `AdminError` /
  `SshSetupError`. The UI surfaces "Coming soon: …" rather than silently
  failing.
- Long subprocess output streams line-by-line into a shared `LogView`.

## Documentation policy

- Do **not** create or edit `*.md` files (including `README.md`,
  `CONTRIBUTING.md`, `SECURITY.md`, `AGENTS.md`, `CLAUDE.md`) unless
  explicitly requested.
- Prefer rustdoc for code documentation.
- When the user asks for README updates, keep it user-facing — pipeline
  details belong in rustdoc or `CONTRIBUTING.md`, not in the README.

## Security rules

These rules are **mandatory**, not suggestions. Most of them prevent
failure modes specific to a maintainer's environment: SSH keys, local
shell, and a push-access remote.

### Shell command construction

- **Never** interpolate package names, file paths, or user input
  directly into shell command strings. Always pass them through
  `Command::new().arg()` — `Command`'s arguments are not interpreted
  by a shell.
- No `sh -c "…"` with user-controlled data. If a multi-step shell
  invocation is required, build the pipeline inside a Rust function
  that spawns each step separately.
- When logging the command for the user's benefit (e.g. `$ makepkg -f
  --noconfirm` in the log view), that string is **display only** — the
  actual invocation still uses discrete `.arg()` calls.

### SSH and key handling

- **Do not overwrite existing key files.** `ensure_aur_key` only runs
  `ssh-keygen -t ed25519 -f ~/.ssh/aur …` when `~/.ssh/aur` does not
  exist. Preserve this invariant.
- **Assert permissions after writes.** `~/.ssh` stays `0700`, the
  private key and `~/.ssh/config` stay `0600`, `~/.ssh/known_hosts`
  stays `0644`. Re-call `fs::set_permissions` even if the tool already
  set them correctly — some editors / umasks mess with them.
- **Never auto-trust host keys silently.** When appending to
  `known_hosts`, capture the SHA256 fingerprint via
  `ssh-keygen -lf -` and surface it as a toast so the user can verify
  against the AUR wiki.
- **Never log private key contents**, session output that contains
  passphrases, or anything else that would embarrass a maintainer on
  screenshot day. Fingerprints (SHA256 prefixes) are fine.

### Network and HTTP

- The PKGBUILD fetcher (`workflow::sync::download_pkgbuild`) uses
  `reqwest` with `rustls-tls` — do not disable TLS verification.
- The AUR RPC (`workflow::aur_account::fetch_my_packages`) validates
  the response via `error_for_status()` before parsing. Don't swallow
  HTTP errors silently.
- `reqwest` with `default-features = false` is deliberate — do not add
  `native-tls` or the default feature set. The `json` feature is on for
  RPC decoding.

### File system safety

- **Validate paths before writing.** `aur_git::aur_clone_dir` and
  `sync::package_dir` resolve paths relative to a trusted work
  directory. When adding new file writers, join against the
  configured work dir rather than dropping into `/tmp` or `$HOME`.
- **Create parent directories explicitly** via
  `tokio::fs::create_dir_all` before writing a child path.
- **JSONC round-trip**: never hand-parse the config or registry files —
  route through `config::read_jsonc`. That strips comments via
  `json_comments::StripComments` before `serde_json` sees the bytes.

### Root and privilege

- `nix_is_root()` in `src/ui/build.rs` is the single source of truth
  for "am I running as root?" before spawning `makepkg`. Don't bypass
  it and don't duplicate the check — extend the helper if you need
  more logic.

### Dependency management

- Prefer direct dependencies over transitive ones for
  security-sensitive functionality.
- Run `cargo update` carefully — pinned major versions in `Cargo.toml`
  (e.g. `gtk4 = "0.11"`, `adw = "0.9"`) match the GTK/libadwaita
  versions available on stable Arch. Bumping them may require bumping
  the feature flags (`v4_14`, `v1_6`).
- Do not add new dependencies that require `unsafe` for their core
  functionality unless there is no safe alternative and the crate is
  well-maintained.

## Complexity and linting (summary)

| Concern | Enforcement | Threshold |
|---------|-------------|-----------|
| Cognitive complexity | `clippy::cognitive_complexity` + `clippy.toml` | 25 |
| Function length | manual review (`too-many-lines-threshold` in `clippy.toml`) | 150 lines |
| Clippy warnings | `-D warnings` on the command line | N/A |
| Licenses / advisories | `cargo deny check` + `deny.toml` | policy |
| Data flow / coupling | manual review | N/A |

## General rules

- No unsolicited `*.md` / wiki / README edits — the `CONTRIBUTING.md`,
  `SECURITY.md`, `AGENTS.md`, and `CLAUDE.md` files only change when
  the user explicitly asks.
- Preserve root-refusal, write-once-key, and fingerprint-surface
  invariants when touching the SSH / build / publish flows.
- Keep typed errors typed — don't collapse `AdminError` or
  `SshSetupError` into `anyhow::Error` at the boundary; the UI code
  matches on the variants to decide whether to show "Coming soon: …"
  vs a concrete failure toast.
