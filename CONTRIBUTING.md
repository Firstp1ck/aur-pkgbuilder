# Contributing to aur-pkgbuilder

Thanks for your interest in contributing! aur-pkgbuilder is a GTK4 +
libadwaita desktop application that walks an AUR maintainer through the
release process end-to-end: login by username, SSH verification, sync,
validate, build, and publish.

For newcomers looking to contribute, we recommend starting with issues
labelled "Good First Issue" in the
[issue tracker](https://github.com/Firstp1ck/aur-pkgbuilder/issues).

## Ways to contribute

- Bug reports and fixes
- Feature requests and implementations
- Documentation improvements
- UI/UX polish and accessibility
- Filling in the `preview`-tagged administration stubs with real
implementations (see `src/workflow/admin.rs`)

## Before you start

- **Target platform**: Arch Linux and Arch-based distributions
(EndeavourOS, Manjaro, CachyOS, Artix). The app shells out to `makepkg`,
`git`, `ssh`, `ssh-keygen`, `ssh-keyscan`, `updpkgsums`, and optionally
`shellcheck`, `namcap`, and `fakeroot`.
- **Safety**: During development, point the app's working directory at a
**disposable path** (e.g. a fresh directory under `/tmp`) so local
experiments can never overwrite real release builds or AUR clones.
- **Security**: If your report involves a security issue, use our
[Security Policy](SECURITY.md).

## Development setup

### Prerequisites

```bash
sudo pacman -S --needed base-devel git gtk4 libadwaita openssh pacman-contrib rustup
rustup default stable
```

Optional, but required to exercise the full validation flow:

```bash
sudo pacman -S --needed shellcheck namcap
```

### Clone and run

```bash
git clone https://github.com/Firstp1ck/aur-pkgbuilder
cd aur-pkgbuilder
cargo run
```

For a release-mode run use `cargo run --release`.

### Tests

```bash
cargo test --bin aur-pkgbuilder
```

The test target is the binary (there is no `lib.rs`), so use
`--bin aur-pkgbuilder` rather than the default.

## Code quality requirements

### Pre-commit checklist

Before committing, ensure all of the following pass:

1. **Format code:**
  ```bash
   cargo fmt --all
  ```
2. **Lint with Clippy** (warnings promoted to errors):
  ```bash
   cargo clippy --all-targets -- -D warnings
  ```
3. **Check compilation:**
  ```bash
   cargo check
  ```
4. **Run tests:**
  ```bash
   cargo test --bin aur-pkgbuilder
  ```

All four must pass cleanly; the CI equivalent will reject anything that
fails.

### Code documentation requirements

For all new code (functions, methods, structs, enums, modules):

1. **Rust documentation comments** are required on public items. Private
  items benefit from docs too — add them when the behavior is
   non-obvious.
2. For non-trivial APIs use the **What / Inputs / Output / Details**
  layout:
3. Narrate intent, not mechanics — don't comment "// increment counter"
  or "// import the module".

### Testing requirements

**For bug fixes:**

1. Create a failing test that reproduces the issue.
2. Fix the bug.
3. Verify the test passes.
4. Add edge-case tests if applicable.

**For new features:**

1. Add unit tests for the core logic.
2. Where state crosses modules (e.g. file parsers, path resolution, SSH
  config upserts), prefer pure functions that are test-friendly — the
   `ssh_setup::upsert_host_block` tests are a good template.
3. Tests must be deterministic; don't rely on network or on the
  developer's home directory.

## Commit and branch guidelines

### Branch naming

- `feat/…` — new features
- `fix/…` — bug fixes
- `docs/…` — documentation only
- `refactor/…` — code refactoring
- `test/…` — test additions/updates
- `chore/…` — build / infrastructure changes

### Commit messages

Prefer [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>: <short summary>

<optional longer description>
```

Types: `feat`, `fix`, `docs`, `refactor`, `perf`, `test`, `chore`, `ui`,
`breaking change`.

Examples:

```
feat: add extended fakeroot validation tier

- Implements check_fakeroot_build and check_namcap_package
- Adds a third preference group on the validate page
- Splits run_all into fast/extended helpers
```

```
fix: preserve file trailing newline when appending to known_hosts
```

Guidelines:

- Keep commits focused and reasonably small.
- Add rationale in the body if the change is non-obvious.
- Reference issues when relevant: `Closes #123` / `Fixes #456`.

## Pull Request process

### Before opening a PR

1. **Quality checks pass:**
  - `cargo fmt --all` (no diff)
  - `cargo clippy --all-targets -- -D warnings` (clean)
  - `cargo check` compiles
  - `cargo test --bin aur-pkgbuilder` green
2. **Code:**
  - New public items have rustdoc
  - No `unwrap()` / `expect()` in non-test code (use proper error
  handling — `anyhow::Result` at the edge, typed errors inside
  `workflow::`*).
  - GTK widgets constructed on the main thread; long work routed
  through `runtime::spawn` or `runtime::spawn_streaming`.
3. **Testing:**
  - Added or updated tests where it makes sense.
  - For bug fixes: a failing test precedes the fix.
4. **Documentation:**
  - README updated if user-visible behavior changed (feature table
   and/or Usage section).
  - If adding a new navigation page, update the step numbering in
  the Usage section.
  - Config schema changes: update the Configuration section of the
  README, including the example snippet.
5. **Compatibility:**
  - Missing external tools degrade gracefully (toast or status row
   saying what to install) — the preflight pattern in
   `src/workflow/preflight.rs` is the reference.
  - Destructive actions are opt-in with clear labels (look at the
  `destructive` pill in `ui/aur_ssh.rs`).
  - Operations never run as root; `makepkg` must stay behind the
  `nix_is_root()` guard in `src/ui/build.rs`.

### PR description

Use this structure in the PR body:

```markdown
## Summary

Brief description of what this PR does.

## Type of change
- [ ] feat (new feature)
- [ ] fix (bug fix)
- [ ] docs (documentation only)
- [ ] refactor (no functional change)
- [ ] perf (performance)
- [ ] test (add/update tests)
- [ ] chore (build/infra/CI)
- [ ] ui (visual/interaction changes)
- [ ] breaking change (incompatible behavior)

## Related issues

Closes #123

## How to test

Step-by-step instructions a reviewer can follow.

## Checklist

- [ ] `cargo fmt --all` clean
- [ ] `cargo clippy --all-targets -- -D warnings` clean
- [ ] `cargo check` compiles
- [ ] `cargo test --bin aur-pkgbuilder` passes
- [ ] README / feature table / Usage section updated if user-visible
      behavior changed
- [ ] For UI changes: screenshots included

## Notes for reviewers

Any additional context, implementation details, or decisions.

## Breaking changes

None (or description if applicable).
```

## Project conventions

### Code style

- **Language**: Rust, edition 2024 (see `Cargo.toml`).
- **Naming**: Clear and descriptive; clarity over brevity.
- **Error handling**: `Result` types at call sites. Typed errors in
`workflow::`* (see `admin::AdminError`, `ssh_setup::SshSetupError`,
`aur_account::AurAccountError`) so UI code can match on specific
failure modes.
- **Early returns** are preferred over deep nesting.
- **Async boundary**: keep GTK objects on the main thread. Use
`runtime::spawn` for one-shot Tokio work and `runtime::spawn_streaming`
for subprocess output that needs to reach the UI log view.

### Project layout

```
src/
  main.rs                 entry + Tokio runtime
  app.rs                  AdwApplicationWindow + first-launch routing
  config.rs               Config (JSONC) + commit-template helpers
  state.rs                AppState shared through Rc<RefCell<..>>
  runtime.rs              Tokio ↔ GLib bridge
  ui/
    home.rs               package list + editor entry + onboarding entry
    onboarding.rs         AUR username → maintained-packages checklist
    connection.rs         tools + SSH probe; opens ssh_setup
    ssh_setup.rs          one-click key/config/known_hosts setup
    sync.rs               download upstream PKGBUILD
    version.rs            updpkgsums + kind-specific guidance
    validate.rs           required / optional / extended check tiers
    build.rs              makepkg -f with streaming log
    publish.rs            clone / diff / commit / push + default template
    manage.rs             admin dashboard (global ops + per-package menu)
    aur_ssh.rs            curated AUR SSH command picker
    package_editor.rs     add/edit a PackageDef
    log_view.rs           shared monospace log widget
  workflow/
    package.rs            PackageDef / PackageKind
    registry.rs           packages.jsonc I/O
    preflight.rs          which + AUR SSH probe
    sync.rs               package_dir helper + HTTP fetch
    build.rs              shared makepkg runner + LogLine
    validate.rs           standard AUR PKGBUILD checks + fakeroot tier
    ssh_setup.rs          ssh-keygen / known_hosts / ssh config upsert
    aur_git.rs            clone / diff / .SRCINFO / commit / push
    aur_account.rs        public AUR RPC lookup
    aur_ssh.rs            typed AUR SSH command wrapper
    admin.rs              placeholder lifecycle ops
```

### UX guidelines

- Follow libadwaita patterns: `PreferencesGroup` for grouped rows,
`ActionRow` + buttons for per-row actions, `Toast` for async results,
`AdwNavigationView` for multi-step flows.
- Destructive actions wear the `destructive` pill and/or
`destructive-action` button class.
- `preview` badge marks intentional stubs that return
`NotImplemented(&'static str)`. The UI surfaces a "Coming soon: …"
toast rather than silently failing.
- Long-running subprocesses stream line-by-line into a shared `LogView`
rather than batching.

### Configuration

- `config.jsonc` and `packages.jsonc` live under
`~/.config/aur-pkgbuilder/`. Both are JSONC — comments survive reads
(via `json_comments::StripComments`) but not writes (the GUI rewrites
the body each save, emitting a fixed header comment).
- If you add a field, add it with `#[serde(default)]` so existing files
upgrade in place. Document it in the header comment (`CONFIG_HEADER`
in `src/config.rs` or `REGISTRY_HEADER` in `src/workflow/registry.rs`).

### Security

- **External data goes through `Command::new().arg()`**, never through
string interpolation into a shell. No `sh -c "…"` with user input.
- **Root guard stays intact**: `makepkg` refuses to run as root. The
`nix_is_root()` check in `src/ui/build.rs` must not be bypassed.
- **Private keys are write-once**: `ensure_aur_key` never overwrites an
existing `~/.ssh/aur`. Preserve that invariant.
- **Known-hosts trust is opt-in**: when we add to `~/.ssh/known_hosts`
we surface the fingerprint so the maintainer can verify it against the
AUR wiki — we do **not** silently pin.
- **Filesystem permissions**: `~/.ssh` → `0700`, private keys and
`~/.ssh/config` → `0600`, `known_hosts` → `0644`. Re-assert perms
after writes even if the subprocess already set them correctly.
- **Never log secrets**: SSH key contents, session output containing
passphrases, etc. must not be copied into toasts or log files
verbatim. Fingerprints (SHA256) are fine.

### Platform behavior

- **Graceful degradation**: every external tool (`makepkg`, `git`,
`ssh`, `updpkgsums`, `shellcheck`, `namcap`, `fakeroot`, `xdg-open`)
may be missing. Surface an actionable install hint
(`pacman -S --needed <pkg>`) instead of crashing.
- **No blocking network on startup**: the main thread stays responsive
while the AUR RPC, SSH probe, and PKGBUILD download run.

## Filing issues

### Bug reports

Include:

- aur-pkgbuilder version (commit hash or release tag).
- Arch-based distribution and version.
- Relevant external tool versions (`makepkg --version`,
`ssh -V`, `namcap --version`, `shellcheck --version`).
- Whether you're on Wayland or X11 and the GTK/libadwaita versions
(`pkg-config --modversion gtk4 libadwaita-1`).
- Steps to reproduce.
- Expected vs. actual behavior.
- Relevant log output. Run with
`GTK_DEBUG=interactive RUST_BACKTRACE=1 cargo run` for more detail.

### Feature requests

- Describe the problem being solved.
- Describe the desired UX/behavior.
- Consider edge cases (missing external tools, first-time use).

Open issues in the
[issue tracker](https://github.com/Firstp1ck/aur-pkgbuilder/issues).

## Security policy

See [SECURITY.md](SECURITY.md) for how to report vulnerabilities.

## Getting help

- Check the [README](README.md) for user-facing documentation.
- Review existing issues and PRs.
- Ask questions in
[Discussions](https://github.com/Firstp1ck/aur-pkgbuilder/discussions)
if enabled.

Thank you for helping improve aur-pkgbuilder.