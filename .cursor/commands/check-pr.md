# check-pr

Check the last commits made by the author of the PR I am reviewing and
explain them extensively. If there are any critical logic errors in the
commits, call them out and suggest fixes. Pay special attention to:

- Root-guard bypasses in `src/ui/build.rs` (`nix_is_root()`).
- Filesystem permission regressions on `~/.ssh/aur`, `~/.ssh/config`,
  or `~/.ssh/known_hosts`.
- New code that interpolates user input directly into shell strings
  instead of using `Command::new().arg()`.
- Config schema changes missing `#[serde(default)]` (would break old
  `config.jsonc` files).
- UI code that mutates GTK widgets from a Tokio task instead of going
  through `runtime::spawn` / `spawn_streaming`.
